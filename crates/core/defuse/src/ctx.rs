//! What a run sees and does.
//!
//! Every read is answered from what is committed now and recorded with its
//! answer; a definition, contribution or placement is kept in the product
//! until the commit. A `Ctx` holds no lock between calls.

use std::sync::Arc;

use crate::api::{At, Effect, FieldAt, Locality, MayDefine, NodeId, OpId, PathId, Step, Stop};
use crate::engine::Engine;
use crate::node::{Anchor, DefRec, Node, Placement, Product, ReadKind, ReadRec, RunRecord, State};
use crate::order::Pos;
use crate::read::{self, SITE, next_value, pos_of, reaching, reaching_at, through};
use crate::sched::Worker;
use crate::values::{Field, FxHashSet, ValueId, Values};

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
    /// Fields whose reads go unrecorded (`Engine::set_stable`).
    pub(crate) stable: Option<&'a FxHashSet<Field>>,
    /// The change count the read in hand loaded ([`ReadRec::epoch`]).
    pub(crate) ep: u64,
    /// The last sibling `following` found, to go on from.
    pub(crate) follow_at: Option<(usize, NodeId)>,
}

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
                effect: Effect::None,
            },
            views: Vec::new(),
            ep: u64::MAX,
            follow_at: None,
            last: me.run_record(),
            stable: worker.map(|w| &*w.stable),
        }
    }

    /// The product, with the output.
    pub(crate) fn into_product(mut self, output: ValueId) -> Product {
        self.product.output = output;
        self.product
    }

    pub fn values(&self) -> &'a Values {
        &self.eng.values
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

    fn recorded(&self, f: Field) -> bool {
        self.stable.is_none_or(|s| !s.contains(&f))
    }

    /// The definition of `f` reaching me, or the environment's value.
    pub fn read(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        self.before_read(f);
        let r = reaching_at(self.eng, f, self.me.id, false).ok_or_else(stop)?;
        if self.recorded(f) {
            self.record(ReadKind::Field { f, at: Anchor::Me }, PathId::WHOLE, r.value);
        }
        Ok(r.value)
    }

    /// The definition of `f` reaching another position.
    pub fn read_at(&mut self, f: Field, at: At) -> Result<FieldAt, Stop> {
        self.check()?;
        self.before_read(f);
        let (anchor, r) = match at {
            At::Before(n) => (Anchor::Before(n), reaching_at(self.eng, f, n, false)),
            At::AfterOwn(n) => (Anchor::AfterOwn(n), reaching_at(self.eng, f, n, true)),
            At::End(u) => (
                Anchor::End(u),
                Some(reaching(self.eng, f, Pos(self.eng.unit(u).end.0 - 1), true)),
            ),
        };
        if self.recorded(f) {
            self.record(ReadKind::Field { f, at: anchor }, PathId::WHOLE, r.and_then(|x| x.value));
        }
        Ok(match r {
            None => FieldAt::Gone,
            Some(x) => match x.value {
                Some(v) => FieldAt::Def(v, x.by),
                None => FieldAt::Unset,
            },
        })
    }

    /// `read` through a path: the run depends only on the path's answer.
    pub fn read_path(&mut self, f: Field, p: PathId) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        self.before_read(f);
        let r = reaching_at(self.eng, f, self.me.id, false).ok_or_else(stop)?;
        let x = through(self.eng, r, p);
        if self.recorded(f) {
            self.record(ReadKind::Field { f, at: Anchor::Me }, p, x);
        }
        Ok(x)
    }

    /// The node whose definition of `f` reaches me (None: the environment).
    pub fn def_site(&mut self, f: Field) -> Result<Option<NodeId>, Stop> {
        self.check()?;
        self.before_read(f);
        let r = reaching_at(self.eng, f, self.me.id, false).ok_or_else(stop)?;
        if self.recorded(f) {
            let seen = through(self.eng, r, SITE);
            self.record(ReadKind::Field { f, at: Anchor::Me }, SITE, seen);
        }
        Ok(r.by)
    }

    /// The environment's value of `f`, whatever is defined before me.
    pub fn env(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        self.before_read(f);
        let x = read::env_value(self.eng, f);
        if self.recorded(f) {
            self.record(ReadKind::Env { f }, PathId::WHOLE, x);
        }
        Ok(x)
    }

    pub fn define(&mut self, f: Field, x: ValueId) {
        self.define_with(f, x, Locality::Local);
    }

    /// `Locality::Scope` and `ScopeAt` are `unimplemented!("scopes: next run")`.
    pub fn define_with(&mut self, f: Field, x: ValueId, loc: Locality) {
        match loc {
            Locality::Local | Locality::Global => {}
            Locality::Scope | Locality::ScopeAt(_) => unimplemented!("scopes: next run"),
        }
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
        self.product.defs.push(DefRec {
            f,
            loc: Locality::Local,
            value: ValueId::UNIT,
            rel: Some(c),
        });
    }

    /// Adds `x` to `f`'s contributions at my position.
    pub fn contribute(&mut self, f: Field, x: ValueId) {
        self.product.contribs.push((f, x));
    }

    fn bound(&self, n: Option<NodeId>) -> Result<Pos, Stop> {
        match n {
            Some(n) => pos_of(self.eng, n).ok_or_else(stop),
            None => Ok(read::unit_start(self.eng, self.me)),
        }
    }

    fn fold_value(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>, incl: bool) -> Result<ValueId, Stop> {
        self.check()?;
        self.before_read(f);
        let fp = self.bound(from)?;
        let tp = match to {
            Some(n) => pos_of(self.eng, n).ok_or_else(stop)?,
            None => pos_of(self.eng, self.me.id).ok_or_else(stop)?,
        };
        Ok(read::fold(self.eng, f, Some(fp), Some(read::end_of(tp, incl))))
    }

    /// The declared monoid's fold over the contributions to `f` after
    /// `from` (None: the unit's start) and before `to` (None: me).
    pub fn fold(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<ValueId, Stop> {
        let x = self.fold_value(f, from, to, false)?;
        self.record(ReadKind::Fold { f, from, to, incl: false }, PathId::WHOLE, Some(x));
        Ok(x)
    }

    /// The fold over the contributions to `f` after `from` (None: the
    /// unit's start) up to and with `to`'s own.
    pub fn fold_through(&mut self, f: Field, from: Option<NodeId>, to: NodeId) -> Result<ValueId, Stop> {
        let x = self.fold_value(f, from, Some(to), true)?;
        self.record(ReadKind::Fold { f, from, to: Some(to), incl: true }, PathId::WHOLE, Some(x));
        Ok(x)
    }

    /// `fold` through a path.
    pub fn fold_path(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>, p: PathId) -> Result<Option<ValueId>, Stop> {
        let x = self.fold_value(f, from, to, false)?;
        let y = read::apply_path(self.eng, Some(x), p);
        self.record(ReadKind::Fold { f, from, to, incl: false }, p, y);
        Ok(y)
    }

    /// The first contribution to `f` after `after` (None: the unit's start),
    /// or None while the next node after it has not committed.
    pub fn next_contribution(&mut self, f: Field, after: Option<NodeId>) -> Result<Option<(NodeId, ValueId)>, Stop> {
        self.check()?;
        self.before_read(f);
        let ap = self.bound(after)?;
        let n = read::next_contribution(self.eng, f, Some(ap));
        self.record(ReadKind::Next { f, after }, PathId::WHOLE, next_value(self.eng, &n));
        Ok(n.item)
    }

    /// A view on `fold(f, from, to)`, read step by step.
    pub fn view(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<View, Stop> {
        let x = self.fold_value(f, from, to, false)?;
        self.views.push(ViewRec {
            kind: ReadKind::Fold { f, from, to, incl: false },
            path: PathId::WHOLE,
            value: Some(x),
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
            value: Some(x),
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

    /// The output my `i`-th placement had at my last commit, if it has run.
    pub fn placed_output(&mut self, i: usize) -> Option<ValueId> {
        let c = self.last.as_ref().and_then(|r| r.placed.get(i).copied());
        let out = c.and_then(|c| {
            let n = self.eng.node(c);
            (n.state() != State::Gone).then(|| n.run_record().map(|r| r.output)).flatten()
        });
        self.record(ReadKind::Placed { i }, PathId::WHOLE, out);
        out
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
        self.product.claim = n;
        crate::commit::apply_claim(self.eng, self.worker, self.me, n);
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

    /// `Effect::None` is accepted; any other is `unimplemented!("scopes: next run")`.
    pub fn set_effect(&mut self, e: Effect) {
        match e {
            Effect::None => self.product.effect = e,
            Effect::Open | Effect::Close | Effect::Scope(_) | Effect::Pending => unimplemented!("scopes: next run"),
        }
    }

    /// Always true: a run never knows it is final. Side effects belong
    /// after the evaluation, from outputs.
    pub fn is_speculative(&self) -> bool {
        true
    }

    pub fn values_epoch(&self) -> u64 {
        self.eng.values.instance()
    }

    /// Forgets my reads of `f` in this run: the commit records none.
    pub fn unread(&mut self, f: Field) {
        self.product.reads.retain(|r| match &r.kind {
            ReadKind::Field { f: g, .. } | ReadKind::Env { f: g } => *g != f,
            ReadKind::Fold { .. } | ReadKind::Next { .. } | ReadKind::Placed { .. } | ReadKind::Following { .. } => true,
        });
    }

    /// A path interned for `read_path` and friends.
    pub fn path(&self, steps: &[Step]) -> PathId {
        self.eng.path(steps)
    }

    /// Accepted and ignored: no fuel, no heartbeat.
    pub fn tick(&mut self, _n: u64) -> Result<(), Stop> {
        self.check()
    }

    /// No scopes span nodes in this core.
    pub fn scope_kind(&mut self) -> Result<Option<ValueId>, Stop> {
        Ok(None)
    }

    pub fn set_kinds(&mut self, _ks: &[ValueId]) {}

    pub fn is_loopback(&mut self, _f: Field) -> bool {
        false
    }

    pub fn declare_loopback(&mut self, _f: Field, _init: ValueId) {
        unimplemented!("loop-back: next run")
    }

    pub fn declare_loopback_early(&mut self, _f: Field, _init: ValueId) {
        unimplemented!("loop-back: next run")
    }

    pub fn loop_part(&mut self, _from: Field, _p: PathId) -> Option<ValueId> {
        unimplemented!("loop-back: next run")
    }

    pub fn define_forward_at(&mut self, _f: Field, _from: Field, _p: PathId, _keep: Option<PathId>, _loc: Locality) -> bool {
        unimplemented!("loop-back: next run")
    }

    pub fn publish(&mut self, _f: Field, _x: ValueId) {
        unimplemented!("loop-back: next run")
    }

    pub fn checkpoint(&mut self, _mark: u32, _state: ValueId) -> Result<(), Stop> {
        unimplemented!("loop-back: next run")
    }

    pub fn restart(&mut self) {
        unimplemented!("loop-back: next run")
    }
}
