//! What a run sees and does.
//!
//! Every read is answered from what is committed now and recorded with its
//! answer; a definition, contribution or placement is kept in the product
//! until the commit. A `Ctx` holds no lock between calls.

use std::sync::Arc;

use crate::api::{Cause, At, Effect, FieldAt, Locality, MayDefine, NodeId, OpId, PathId, Step, Stop};
use crate::engine::Engine;
use crate::node::{Anchor, DefRec, Node, Placement, Product, ReadKind, ReadRec, RunRecord, State};
use crate::order::Pos;
use crate::read::{self, SITE, next_value, pos_of, reaching_at, through};
use crate::sched::Worker;
use crate::values::{Field, FxHashMap, ValueId, Values};

/// A handle on a value read through a path, extended step by step
/// (`Ctx::at`) and read at a step (`Ctx::get`): the run depends only on
/// what it took.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct View(u32);

pub(crate) struct ViewRec {
    pub(crate) kind: ReadKind,
    pub(crate) path: PathId,
    /// The value under the path when the view was made.
    pub(crate) value: Option<ValueId>,
    /// Its field's change count before the view was made.
    pub(crate) epoch: u64,
}

pub struct Ctx<'a> {
    pub(crate) eng: &'a Engine,
    pub(crate) worker: Option<&'a Worker>,
    pub(crate) me: &'a Arc<Node>,
    pub(crate) args: Arc<[ValueId]>,
    pub(crate) product: Product,
    pub(crate) views: Vec<ViewRec>,
    /// Its last record, for `placed_output`.
    pub(crate) last: Option<Arc<RunRecord>>,
    /// The change count the read in hand loaded ([`ReadRec::epoch`]).
    pub(crate) ep: u64,
    /// The last sibling `following` found, to go on from.
    pub(crate) follow_at: Option<(usize, NodeId)>,
    /// What `read` answered in this run, by field: a field read again is
    /// answered from here, with no second lookup and no second record.
    pub(crate) seen: FxHashMap<Field, Option<ValueId>>,
    /// Where the last `next_contribution` that found none stopped.
    pub(crate) stop_at: Option<NodeId>,
    /// Why this run was asked for, as it began (`woken_by`): a cause a
    /// re-ask stores while it runs is the next run's.
    pub(crate) woke: Option<Cause>,
    /// My last run's placements by operation and arguments, made on the
    /// first `placed_output_or`.
    placed_keys: Option<PlacedKeys>,
    /// Why the run said it is stuck (`Ctx::stuck`).
    pub(crate) stuck: Option<String>,
    /// Its checks so far ([`Ctx::tick`]).
    ticks: u64,
}

/// The check ([`Ctx::tick`]) from which a run's reads are looked at
/// again: a TeX run's 64 checks are a quarter of a million commands,
/// past the longest run of a document's piece or page, so ordinary runs
/// are never looked at, and a run that goes on for ever is looked at a
/// logarithmic number of times and after each re-ask.
const LONG_RUN_TICKS: u64 = 64;

/// A run's placements by operation and arguments.
type PlacedKeys = FxHashMap<(OpId, Arc<[ValueId]>), NodeId>;

fn stop() -> Stop {
    Stop { _private: () }
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(eng: &'a Engine, worker: Option<&'a Worker>, me: &'a Arc<Node>) -> Ctx<'a> {
        Ctx {
            eng,
            worker,
            me,
            args: me.args(),
            product: Product {
                output: ValueId::UNIT,
                defs: Vec::new(),
                contribs: Vec::new(),
                reads: Vec::new(),
                placements: Vec::new(),
                claim: 0,
                effect: None,
                scope_gen: eng.scope_gen.load(std::sync::atomic::Ordering::Acquire),
                unchanged: false,
                state: None,
                resumed: std::cell::Cell::new(false),
                woken_at: None,
                settle_now: false,
            },
            views: Vec::new(),
            ep: u64::MAX,
            follow_at: None,
            seen: FxHashMap::default(),
            stop_at: None,
            woke: me.woke.lock().as_ref().map(|w| w.cause.clone()),
            placed_keys: None,
            last: me.run_record(),
            stuck: None,
            ticks: 0,
        }
    }

    /// The product, with the output.
    pub(crate) fn into_product(mut self, output: ValueId) -> Product {
        self.product.output = output;
        self.product.woken_at = match self.woke {
            Some(Cause::Range { by, .. } | Cause::Changed { site: Some(by), .. }) => Some(by),
            _ => None,
        };
        self.product
    }

    pub fn values(&self) -> &'a Values {
        &self.eng.values
    }

    /// My unit: a read at its end (`At::End`).
    pub fn unit(&self) -> crate::api::UnitId {
        self.me.unit
    }

    pub fn node(&self) -> NodeId {
        self.me.id
    }

    pub fn op(&self) -> OpId {
        self.me.op()
    }

    pub fn args(&self) -> &[ValueId] {
        &self.args
    }

    /// `Err(Stop)` once a commit has refuted a read of this run.
    fn check(&self) -> Result<(), Stop> {
        match self.me.state() {
            State::RunningDirty | State::Inert | State::RunningInert | State::Gone => Err(stop()),
            State::Clean | State::Dirty | State::Running => Ok(()),
        }
    }

    fn record(&mut self, kind: ReadKind, path: PathId, seen: Option<ValueId>) {
        let epoch = if kind.field().is_some() { self.ep } else { u64::MAX };
        self.product.reads.push(ReadRec { kind, path, seen, epoch });
    }

    /// Loads `f`'s change count before a read of it ([`ReadRec::epoch`]).
    fn before_read(&mut self, f: Field) {
        self.ep = self.eng.field_epoch(f);
    }

    /// A family snapshot (docs/spec-state-model.md §2.1, §9): the value
    /// folding the reaching definitions of every field whose name starts
    /// with `family`'s name (`cat:`, `param:`, `count:` …) at my position,
    /// as a list of `[field, value]` pairs in field order; unrecorded (no
    /// read is filed), cached by content in the store. A client loads a
    /// whole table from it and records only the items it uses
    /// (`read_item`). The body is the rewrite's; the test is
    /// tests/snapshot.rs.
    pub fn snapshot(&mut self, family: Field) -> Result<ValueId, Stop> {
        self.check()?;
        let v = self.values();
        let Some(prefix) = v.as_str(family.0) else {
            return Ok(v.list(Vec::new()));
        };
        let has = |f: Field| v.with_str(f.0, |s| s.starts_with(prefix.as_str())).unwrap_or(false);
        // (the family's fields: those with a definition filed in some
        // segment, and those the environment holds now; one reaching me
        // is defined in a segment before me or is the environment's)
        let mut fields: Vec<Field> = Vec::new();
        for sid in self.eng.order.all() {
            let seg = self.eng.seg(sid);
            let inner = seg.inner.read();
            fields.extend(inner.defs.keys().copied().filter(|&f| has(f)));
        }
        fields.extend(self.eng.env.values.read().keys().copied().filter(|&f| has(f)));
        fields.sort_unstable_by_key(|f| f.0.raw());
        fields.dedup();
        let mut pairs = Vec::with_capacity(fields.len());
        for f in fields {
            let r = reaching_at(self.eng, f, self.me.id, false).ok_or_else(stop)?;
            if let Some(x) = r.value {
                pairs.push(v.list(vec![f.0, x]));
            }
        }
        // (interned by content: the same definitions reaching give the
        // same value wherever they reach)
        Ok(v.list(pairs))
    }

    /// Records a read of `f` answered `seen`, as `read` would have
    /// recorded it, without answering it again: a client that loaded
    /// `f`'s value from a snapshot and used it files the read here, so a
    /// definition of `f` reaching me that differs from `seen` re-asks me
    /// (docs/spec-state-model.md §2.1).
    pub fn read_item(&mut self, f: Field, seen: Option<ValueId>) -> Result<(), Stop> {
        self.check()?;
        if self.seen.contains_key(&f) {
            return Ok(());
        }
        // (no change count: the value was loaded before now, so the
        // commit checks the read whatever its field's count says)
        self.ep = u64::MAX;
        self.record(ReadKind::Field { f, at: Anchor::Me }, PathId::WHOLE, seen);
        self.seen.insert(f, seen);
        Ok(())
    }

    /// The definition of `f` reaching me, or the environment's value; an
    /// alias definition followed ([`Ctx::define_alias`]).
    pub fn read(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        if let Some(&x) = self.seen.get(&f) {
            return Ok(x);
        }
        self.before_read(f);
        let r = reaching_at(self.eng, f, self.me.id, false).ok_or_else(stop)?;
        self.record(ReadKind::Field { f, at: Anchor::Me }, PathId::WHOLE, r.value);
        let x = self.follow_aliases(r.value)?;
        self.seen.insert(f, x);
        Ok(x)
    }

    /// `x` with each alias definition it is followed: a read of the
    /// aliased field before the alias's node, recorded as mine
    /// ([`Ctx::define_alias`]).
    fn follow_aliases(&mut self, mut x: Option<ValueId>) -> Result<Option<ValueId>, Stop> {
        for _ in 0..32 {
            let Some((g, at)) = x.and_then(|y| read::alias_of(self.values(), y)) else {
                return Ok(x);
            };
            x = match self.read_at_raw(g, At::Before(at))? {
                FieldAt::Def(y, _) => Some(y),
                FieldAt::Unset | FieldAt::Gone => None,
            };
        }
        Ok(x)
    }

    /// `f` := the value of `g` reaching node `at` (`At::Before(at)`), an
    /// alias: a reader of `f` that this definition reaches is answered
    /// with that value and reads `g` there itself, so a change of `g`'s
    /// definition reaching `at` asks that reader again and never me; an
    /// alias of an alias is followed. For a value a run passes on without
    /// looking at it (a page-layer register's symbol copied into another
    /// register, docs/spec-state-model.md §2.2).
    pub fn define_alias(&mut self, f: Field, g: Field, at: NodeId, loc: Locality) {
        let x = read::alias_value(self.values(), g, at);
        self.product.defs.push(DefRec { f, loc, value: x, rel: None });
    }

    /// The definition of `f` reaching another position; an alias
    /// definition followed ([`Ctx::define_alias`]).
    pub fn read_at(&mut self, f: Field, at: At) -> Result<FieldAt, Stop> {
        let r = self.read_at_raw(f, at)?;
        if let FieldAt::Def(x, by) = r
            && read::alias_of(self.values(), x).is_some()
        {
            return Ok(match self.follow_aliases(Some(x))? {
                Some(y) => FieldAt::Def(y, by),
                None => FieldAt::Unset,
            });
        }
        Ok(r)
    }

    /// [`Ctx::read_at`] with an alias definition as it is.
    fn read_at_raw(&mut self, f: Field, at: At) -> Result<FieldAt, Stop> {
        self.check()?;
        self.before_read(f);
        let (anchor, r) = match at {
            At::Before(n) => (Anchor::Before(n), reaching_at(self.eng, f, n, false)),
            At::AfterOwn(n) => (Anchor::AfterOwn(n), reaching_at(self.eng, f, n, true)),
            At::Final => (Anchor::Final, Some(crate::read::final_read(self.eng, f))),
            At::End(u) => (
                Anchor::End(u),
                Some(read::reaching_visible(self.eng, f, Pos(self.eng.unit(u).end.0 - 1), true)),
            ),
        };
        self.record(ReadKind::Field { f, at: anchor }, PathId::WHOLE, r.and_then(|x| x.value));
        Ok(match r {
            None => FieldAt::Gone,
            Some(x) => match x.value {
                Some(v) => FieldAt::Def(v, x.by),
                None => FieldAt::Unset,
            },
        })
    }

    /// `read` through a path: the run depends only on the path's answer
    /// (an alias definition: on all of it, followed).
    pub fn read_path(&mut self, f: Field, p: PathId) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        self.before_read(f);
        let r = reaching_at(self.eng, f, self.me.id, false).ok_or_else(stop)?;
        if let Some(x) = r.value
            && read::alias_of(self.values(), x).is_some()
        {
            self.record(ReadKind::Field { f, at: Anchor::Me }, PathId::WHOLE, r.value);
            let y = self.follow_aliases(r.value)?;
            return Ok(read::apply_path(self.eng, y, p));
        }
        let x = through(self.eng, r, p);
        self.record(ReadKind::Field { f, at: Anchor::Me }, p, x);
        Ok(x)
    }

    /// The node whose definition of `f` reaches me (None: the environment).
    pub fn def_site(&mut self, f: Field) -> Result<Option<NodeId>, Stop> {
        self.check()?;
        self.before_read(f);
        let r = reaching_at(self.eng, f, self.me.id, false).ok_or_else(stop)?;
        let seen = through(self.eng, r, SITE);
        self.record(ReadKind::Field { f, at: Anchor::Me }, SITE, seen);
        Ok(r.by)
    }

    /// The environment's value of `f`, whatever is defined before me.
    pub fn env(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        self.before_read(f);
        let x = read::env_value(self.eng, f);
        self.record(ReadKind::Env { f }, PathId::WHOLE, x);
        Ok(x)
    }

    pub fn define(&mut self, f: Field, x: ValueId) {
        self.define_with(f, x, Locality::Local);
    }

    /// A definition at the locality's depth (`Locality`): the commit
    /// files it with its depth, from my brackets (`set_effect`, or my
    /// operation's `effect`) and the depth reaching me.
    pub fn define_with(&mut self, f: Field, x: ValueId, loc: Locality) {
        self.product.defs.push(DefRec {
            f,
            loc,
            value: x,
            rel: None,
        });
    }

    /// `f` := the integer definition of `f` reaching me, plus `c`. Kept as
    /// a relation: when its input changes the engine recomputes it without
    /// running me, and once this operation has made one, its pending nodes
    /// are given it ahead of their runs. This is how a counter chain is not
    /// a chain of runs. The run need not read `f` first.
    pub fn define_rel(&mut self, f: Field, c: i64) {
        self.define_rel_with(f, c, Locality::Local);
    }

    /// [`Ctx::define_rel`] at a locality: `Global`, the value reaching me
    /// (the innermost binding seen here) plus `c`, standing past the
    /// scopes I am in (a global `\advance` inside an environment).
    pub fn define_rel_with(&mut self, f: Field, c: i64, loc: Locality) {
        self.product.defs.push(DefRec {
            f,
            loc,
            value: ValueId::UNIT,
            rel: Some(c),
        });
    }

    /// Adds `x` to `f`'s contributions at my position.
    pub fn contribute(&mut self, f: Field, x: ValueId) {
        self.product.contribs.push((f, x));
    }

    /// A bound's label: None when the node named is gone (the read
    /// answers none, as a read at a gone node does, `FieldAt::Gone`).
    fn bound(&self, n: Option<NodeId>) -> Option<Pos> {
        match n {
            Some(n) => pos_of(self.eng, n),
            None => Some(read::unit_start(self.eng, self.me)),
        }
    }

    /// The fold's value; None where a bound is gone.
    fn fold_value(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>, incl: bool) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        self.before_read(f);
        let Some(fp) = self.bound(from) else {
            return Ok(None);
        };
        let tp = match to {
            Some(n) => pos_of(self.eng, n),
            None => Some(pos_of(self.eng, self.me.id).ok_or_else(stop)?),
        };
        if let Some(tp) = tp {
            read::trace_inverted(self.eng, "run", self.me, f, (from, fp), (to, read::end_of(tp, incl)));
        }
        Ok(tp.map(|tp| read::fold(self.eng, f, Some(fp), Some(read::end_of(tp, incl)))))
    }

    /// The monoid's empty value: a fold whose bound is gone.
    fn empty(&self, f: Field) -> ValueId {
        self.eng.monoid(f).empty(self.values())
    }

    /// The declared monoid's fold over the contributions to `f` after
    /// `from` (None: the unit's start) and before `to` (None: me); empty
    /// where `from` or `to` is gone.
    pub fn fold(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<ValueId, Stop> {
        let x = self.fold_value(f, from, to, false)?;
        self.record(ReadKind::Fold { f, from, to, incl: false }, PathId::WHOLE, x);
        Ok(x.unwrap_or_else(|| self.empty(f)))
    }

    /// The fold over the contributions to `f` after `from` (None: the
    /// unit's start) up to and with `to`'s own.
    pub fn fold_through(&mut self, f: Field, from: Option<NodeId>, to: NodeId) -> Result<ValueId, Stop> {
        let x = self.fold_value(f, from, Some(to), true)?;
        self.record(ReadKind::Fold { f, from, to: Some(to), incl: true }, PathId::WHOLE, x);
        Ok(x.unwrap_or_else(|| self.empty(f)))
    }

    /// `fold` through a path.
    pub fn fold_path(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>, p: PathId) -> Result<Option<ValueId>, Stop> {
        let x = self.fold_value(f, from, to, false)?;
        let y = x.and_then(|x| read::apply_path(self.eng, Some(x), p));
        self.record(ReadKind::Fold { f, from, to, incl: false }, p, y);
        Ok(y)
    }

    /// The first contribution to `f` after `after` (None: the unit's start),
    /// or None while the next node after it has not committed, or where
    /// `after` is gone.
    pub fn next_contribution(&mut self, f: Field, after: Option<NodeId>) -> Result<Option<(NodeId, ValueId)>, Stop> {
        self.check()?;
        self.before_read(f);
        let Some(ap) = self.bound(after) else {
            self.record(ReadKind::Next { f, after }, PathId::WHOLE, None);
            return Ok(None);
        };
        let n = read::next_contribution(self.eng, f, Some(ap));
        self.record(ReadKind::Next { f, after }, PathId::WHOLE, next_value(self.eng, &n));
        self.stop_at = if n.item.is_none() { n.stop } else { None };
        Ok(n.item)
    }

    /// The node the last `next_contribution` that answered none stopped
    /// at, one that has not committed yet; None where it met the line's
    /// end (or a gone bound).
    pub fn stopped_at(&self) -> Option<NodeId> {
        self.stop_at
    }

    /// A view on `fold(f, from, to)`, read step by step.
    pub fn view(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<View, Stop> {
        let x = self.fold_value(f, from, to, false)?;
        self.views.push(ViewRec {
            kind: ReadKind::Fold { f, from, to, incl: false },
            path: PathId::WHOLE,
            value: x,
            epoch: self.ep,
        });
        Ok(View(u32::try_from(self.views.len() - 1).unwrap_or(u32::MAX)))
    }

    /// A view on `fold_through(f, from, to)`.
    pub fn view_through(&mut self, f: Field, from: Option<NodeId>, to: NodeId) -> Result<View, Stop> {
        let x = self.fold_value(f, from, Some(to), true)?;
        self.views.push(ViewRec {
            kind: ReadKind::Fold { f, from, to: Some(to), incl: true },
            path: PathId::WHOLE,
            value: x,
            epoch: self.ep,
        });
        Ok(View(u32::try_from(self.views.len() - 1).unwrap_or(u32::MAX)))
    }

    /// A view on `read(f)`.
    pub fn view_field(&mut self, f: Field) -> Result<View, Stop> {
        self.check()?;
        self.before_read(f);
        let r = reaching_at(self.eng, f, self.me.id, false).ok_or_else(stop)?;
        self.views.push(ViewRec {
            kind: ReadKind::Field { f, at: Anchor::Me },
            path: PathId::WHOLE,
            value: r.value,
            epoch: self.ep,
        });
        Ok(View(u32::try_from(self.views.len() - 1).unwrap_or(u32::MAX)))
    }

    fn view_rec(&self, v: View) -> Option<&ViewRec> {
        self.views.get(v.0 as usize)
    }

    /// The view one step further; None where the step does not apply.
    pub fn at(&mut self, v: View, s: Step) -> Option<View> {
        let rec = self.view_rec(v)?;
        let (kind, value, epoch) = (rec.kind.clone(), rec.value, rec.epoch);
        let path = self.eng.path_extend(rec.path, s);
        let x = read::step(self.eng, value, s);
        let present = self.eng.path_extend(path, Step::Present);
        let seen = Some(self.eng.values().bool(x.is_some()));
        self.ep = epoch;
        self.record(kind.clone(), present, seen);
        x?;
        self.views.push(ViewRec { kind, path, value: x, epoch });
        Some(View(u32::try_from(self.views.len() - 1).unwrap_or(u32::MAX)))
    }

    /// The value at one step further, recorded as a read of that path.
    pub fn get(&mut self, v: View, s: Step) -> Option<ValueId> {
        let rec = self.view_rec(v)?;
        let (kind, value, epoch) = (rec.kind.clone(), rec.value, rec.epoch);
        let path = self.eng.path_extend(rec.path, s);
        let x = read::step(self.eng, value, s);
        self.ep = epoch;
        self.record(kind, path, x);
        x
    }

    pub fn len(&mut self, v: View) -> Option<u64> {
        let x = self.get(v, Step::Len)?;
        self.eng.values().as_int(x).and_then(|i| u64::try_from(i).ok())
    }

    pub fn has(&mut self, v: View, k: ValueId) -> bool {
        self.get(v, Step::Has(k)).and_then(|x| self.eng.values().as_bool(x)).unwrap_or(false)
    }

    /// The whole value under the view, recorded as a read of all of it.
    pub fn whole(&mut self, v: View) -> ValueId {
        let Some(rec) = self.view_rec(v) else {
            return ValueId::UNIT;
        };
        let (kind, path, value) = (rec.kind.clone(), rec.path, rec.value);
        self.ep = rec.epoch;
        self.record(kind, path, value);
        value.unwrap_or(ValueId::UNIT)
    }

    /// Places a node after me, before my continuation and next sibling;
    /// its index among my placements.
    pub fn child(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        self.product.placements.push(Placement::Child(op, args));
        self.product.placements.len() - 1
    }

    /// Places a node after me and my children.
    pub fn follow(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        self.product.placements.push(Placement::Follow(op, args));
        self.product.placements.len() - 1
    }

    /// Places a node after `at` and its placements.
    pub fn follow_after(&mut self, at: NodeId, op: OpId, args: Vec<ValueId>) -> usize {
        self.product.placements.push(Placement::FollowAfter(at, op, args));
        self.product.placements.len() - 1
    }

    /// Places a node after the node my `j`-th placement of this run
    /// stands for, and its placements: a run placing several nodes after
    /// one node elsewhere puts each after the one before, in its order
    /// (`follow_after` puts each right after that node's own children).
    ///
    /// # Panics
    ///
    /// Where `j` is not a placement this run made before.
    pub fn follow_after_placed(&mut self, j: usize, op: OpId, args: Vec<ValueId>) -> usize {
        assert!(j < self.product.placements.len(), "follow_after_placed({j}): no such placement before");
        self.product.placements.push(Placement::FollowAfterPlaced(j, op, args));
        self.product.placements.len() - 1
    }

    /// The output my `i`-th placement had at my last commit, if it has run.
    pub fn placed_output(&mut self, i: usize) -> Option<ValueId> {
        let c = self.last.as_ref().and_then(|r| r.placed.get(i).copied());
        let out = c.and_then(|c| {
            let n = self.eng.node(c);
            (n.state() != State::Gone).then(|| n.run_record().map(|r| r.output)).flatten()
        });
        self.record(ReadKind::Placed { i, guess: false }, PathId::WHOLE, out);
        out
    }

    /// The output of the node my `i`-th placement of this run stands for
    /// (my last run's of the same operation and arguments) if it has run,
    /// else `guess`: recorded as seen, so a placement whose first output
    /// is the guess re-asks me for nothing, and one that differs re-asks
    /// me (a loop folding a child's result goes on on its guess).
    pub fn placed_output_or(&mut self, i: usize, guess: ValueId) -> ValueId {
        if self.placed_keys.is_none() {
            let mut m: FxHashMap<(OpId, Arc<[ValueId]>), NodeId> = FxHashMap::default();
            for &c in self.last.as_ref().map_or(&[][..], |r| &r.placed[..]) {
                if let Some(n) = self.eng.placed_node(c).filter(|n| n.state() != State::Gone) {
                    m.entry((n.op(), n.args())).or_insert(c);
                }
            }
            self.placed_keys = Some(m);
        }
        let out = self.product.placements.get(i).and_then(|p| {
            let (op, args) = p.key();
            let c = *self.placed_keys.as_ref()?.get(&(op, Arc::from(args)))?;
            let n = self.eng.node(c);
            (n.state() != State::Gone).then(|| n.run_record().map(|r| r.output)).flatten()
        });
        let x = out.unwrap_or(guess);
        self.record(ReadKind::Placed { i, guess: true }, PathId::WHOLE, Some(x));
        x
    }

    /// `op` on `args` inline: the output. It reads at my position and its
    /// reads are mine. A run that read nothing is memoised by (op, args);
    /// one that read depends on where it ran, and runs at each caller.
    pub fn apply(&mut self, op: OpId, args: Vec<ValueId>) -> Result<ValueId, Stop> {
        self.check()?;
        let key = (op, args.clone().into_boxed_slice());
        if let Some(&x) = self.eng.memo.read().get(&key) {
            return Ok(x);
        }
        let o = self.eng.op(op).clone();
        let mut sub = Ctx::new(self.eng, self.worker, self.me);
        sub.args = args.into();
        let out = o.run(&mut sub)?;
        if sub.product.reads.is_empty() {
            self.eng.memo.write().insert(key, out);
        } else {
            self.product.reads.append(&mut sub.product.reads);
        }
        Ok(out)
    }

    /// Takes my next `n` siblings: they become inert and `following` gives
    /// their (op, args). Pushed siblings only, not placements.
    pub fn claim(&mut self, n: usize) {
        self.me.claim_all.store(false, std::sync::atomic::Ordering::Release);
        self.product.claim = n;
        crate::commit::apply_claim(self.eng, self.worker, self.me, n);
    }

    /// Claims every text node (`Operation::text`) after this one in its
    /// unit, whatever placed it (not its pushed siblings alone, as `claim`
    /// does): the job is over here (TeX's fatal error, §93: nothing after
    /// it is read). The nodes are inert, their products withdrawn, until a
    /// run of this node claims otherwise; what is not text (the writers
    /// after the text) stays and runs on what was made before. A fatal error in a paper's preamble claimed its one
    /// pushed sibling and left the 11000 pieces of the included files to
    /// run on, 65 times each at ten threads.
    pub fn claim_rest(&mut self) {
        self.me.claim_all.store(true, std::sync::atomic::Ordering::Release);
        self.product.claim = usize::MAX;
        crate::commit::apply_claim(self.eng, self.worker, self.me, usize::MAX);
    }

    /// The `i`-th claimed sibling's (op, args).
    pub fn following(&mut self, i: usize) -> Option<(OpId, Vec<ValueId>)> {
        let n = read::following_on(self.eng, self.me, i, &mut self.follow_at);
        let seen = n.as_ref().map(|n| read::text_value(self.eng, n));
        self.record(ReadKind::Following { i }, PathId::WHOLE, seen);
        n.map(|n| (n.op(), n.args().to_vec()))
    }

    /// Narrows what I may define, for prediction. Accepted, not used by
    /// this core.
    pub fn narrow(&mut self, _m: MayDefine) {}

    /// My scope brackets, when they are known only by running
    /// (`Operation::effect` gave `Pending`). The commit files them; the
    /// readers after me whose answer moved are re-asked.
    pub fn set_effect(&mut self, e: Effect) {
        self.product.effect = Some(e);
    }

    /// The scope depth I start at: scopes open just before me, my own
    /// closes not applied. Recorded: a bracket before me that changes it
    /// runs me again.
    pub fn depth(&mut self) -> Result<u32, Stop> {
        self.check()?;
        let f = self.eng.depth_field;
        self.before_read(f);
        let d = read::depth_of(self.eng, self.me.id).ok_or_else(stop)?;
        let x = self.eng.values().int(i64::from(d));
        self.record(ReadKind::Field { f, at: Anchor::Me }, PathId::WHOLE, Some(x));
        Ok(d)
    }

    /// Always true: a run never knows it is final. Side effects belong
    /// after the evaluation, from outputs.
    pub fn is_speculative(&self) -> bool {
        true
    }

    /// The value store's epoch: its instance and the collections it has
    /// made ([`crate::Values::collect`]). An id a client cached at one
    /// epoch is valid while the epoch lasts: one a collection freed is
    /// never handed out again.
    pub fn values_epoch(&self) -> u64 {
        self.eng.values.epoch()
    }

    /// This run publishes nothing new: what my last run defined,
    /// contributed, placed and output stands as it is, and only this
    /// run's reads are filed, so a change to one (a node it stopped at
    /// committing) runs me again. For a run that met what is not there
    /// yet (a stop at a hole) before it could say anything: what it
    /// said before is the best guess until then, and taking it back
    /// would re-ask every reader of it, and remove every placement and
    /// theirs, to put them back once the hole fills. On a first run there
    /// is nothing to keep (a prediction stands as made).
    pub fn unchanged(&mut self) {
        self.product.unchanged = true;
    }

    /// What this run defines is no decision of its settling layer
    /// (`Operation::settles_at_quiet`) but a step of a serial chain the
    /// text waits on: a changed definition re-asks a reader outside the
    /// layer at once, as it re-asks one inside it, instead of holding the
    /// wake to the next quiet point (docs/spec-settle.md §2). Kept with
    /// the run's record: a run that publishes nothing new
    /// (`Ctx::unchanged`) keeps its last run's.
    pub fn settle_now(&mut self) {
        self.product.settle_now = true;
    }

    /// Stores `state` with this run's record for my next run (`resumed`):
    /// a long loop (the page builder over the whole text) that meets a
    /// hole commits what it has so far, definitions and placements, and
    /// goes on from here when the hole fills, instead of from its start.
    /// The state is a value of mine, no read: nothing re-asks me for it.
    pub fn park(&mut self, state: ValueId) {
        self.product.state = Some(state);
    }

    /// What my last run stored with `park`, if it did; `None` on a first
    /// run, after a run that stored nothing, and after an edit of my text.
    /// A run that takes it goes on from what the last run read: those
    /// reads are kept with this run's (the ones it reads again replaced),
    /// so a change behind where it resumes still asks it again.
    pub fn resumed(&self) -> Option<ValueId> {
        if matches!(self.woken_by(), Some(Cause::Edited)) || self.me.state_stale.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        let st = self.last.as_ref().and_then(|r| r.state);
        if st.is_some() {
            self.product.resumed.set(true);
        }
        st
    }

    /// Whether node `a` stands before node `b` on the line (both alive). A
    /// loop resumed after a contribution moved asks whether the mover lies
    /// behind its cursor (then it goes back to the checkpoint before it)
    /// or ahead (then it goes on). Not a read.
    pub fn before(&self, a: NodeId, b: NodeId) -> bool {
        self.eng.node(a).pos_hint() < self.eng.node(b).pos_hint()
    }

    /// Whether node `n` still stands (placed, not removed): a run about
    /// to place a node after `n` (`follow_after`) asks first. Not a read.
    pub fn alive(&self, n: NodeId) -> bool {
        self.eng.placed_node(n).is_some_and(|x| x.state() != State::Gone)
    }

    /// Why this run was asked for: the cause that marked me dirty (a
    /// definition or contribution I read that moved, with its site; my
    /// creation; an edit). A loop resumes from the checkpoint before the
    /// site. `None` where the engine recorded none.
    pub fn woken_by(&self) -> Option<Cause> {
        self.woke.clone()
    }

    /// Forgets my reads of `f` in this run: the commit records none.
    pub fn unread(&mut self, f: Field) {
        self.seen.remove(&f);
        self.product.reads.retain(|r| match &r.kind {
            ReadKind::Field { f: g, .. } | ReadKind::Env { f: g } | ReadKind::Fold { f: g, .. } | ReadKind::Next { f: g, .. } => *g != f,
            ReadKind::Placed { .. } | ReadKind::Following { .. } => true,
        });
    }

    /// A path interned for `read_path` and friends.
    pub fn path(&self, steps: &[Step]) -> PathId {
        self.eng.path(steps)
    }

    /// The run's periodic check (a client calls it every so much work,
    /// a TeX run every 4096 commands): `Stop` when the run is refuted
    /// (`RunningDirty`); and from its 64th check on, a long run, when one
    /// of its reads so far no longer holds, looked at at every power of
    /// two and at the next check after a re-ask came while it ran
    /// (`sched::mark_stale` leaves a running node to its commit). A run
    /// on what moved may never end (a TeX loop on a stale page or float
    /// state) and so never reach the commit that would refute it, and a
    /// run's first reads are filed nowhere a walk could re-ask it by. A
    /// run whose reads all hold goes on; a short run is left to its
    /// commit, as before.
    pub fn tick(&mut self, _n: u64) -> Result<(), Stop> {
        self.check()?;
        self.ticks += 1;
        if self.ticks >= LONG_RUN_TICKS
            && (self.ticks.is_power_of_two() || self.me.stale.load(std::sync::atomic::Ordering::Acquire))
        {
            self.recheck()?;
        }
        Ok(())
    }

    /// [`Ctx::tick`]'s look at the reads: the run marked dirty (for the
    /// re-ask that came, if one) and stopped if one moved; as the
    /// commit's check, a field no change has counted since it was read
    /// stands, and a sibling's read is left to the commit.
    #[cold]
    fn recheck(&mut self) -> Result<(), Stop> {
        use std::sync::atomic::Ordering;
        self.me.stale.store(false, Ordering::Release);
        let wake = self.me.stale_wake.lock().take();
        let scope_moved = read::scoped(self.eng)
            && self.eng.scope_gen.load(Ordering::Acquire) != self.product.scope_gen;
        let mut memo = crate::read::FoldMemo::new();
        let moved = self.product.reads.iter().any(|rd| {
            if matches!(rd.kind, ReadKind::Following { .. }) {
                return false;
            }
            if !scope_moved
                && !matches!(rd.kind, ReadKind::Next { .. })
                && rd.kind.field().is_some_and(|f| self.eng.field_epoch(f) == rd.epoch)
            {
                return false;
            }
            !crate::read::holds(self.eng, self.me, rd, &mut memo)
        });
        if !moved {
            return Ok(());
        }
        match wake {
            Some(wake) => {
                crate::sched::mark_dirty_now(self.eng, self.worker, self.me, wake);
            }
            None => {
                let _ = self.me.cas_state(State::Running, State::RunningDirty);
            }
        }
        if let Some(w) = self.worker {
            Worker::add(&w.stopped_stale, 1);
        }
        Err(stop())
    }

    /// The run makes no progress (`why`, the client's measure against
    /// `Limits::stall` and `Limits::memory_bytes`): the `Stop` to return,
    /// and the evaluation ends with `EvalError::Stuck` naming this node.
    pub fn stuck(&mut self, why: String) -> Stop {
        self.stuck = Some(why);
        stop()
    }

    /// The engine's limits (`Engine::set_limits`).
    pub fn limits(&self) -> crate::Limits {
        self.eng.limits()
    }

    /// How long the evaluation running this has been going (`None` for a
    /// run outside one): a run checks `Limits::time` itself, which the
    /// scheduler checks only between runs.
    pub fn elapsed(&self) -> Option<std::time::Duration> {
        self.worker.map(|w| w.shared.t0.elapsed())
    }

}
