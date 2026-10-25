//! What a run sees and does.
//!
//! Every read is answered from what is committed now and recorded with its
//! answer; a definition, contribution or placement is kept in the product
//! until the commit. A `Ctx` holds no lock between calls.

use std::sync::Arc;

use crate::api::{At, Effect, FieldAt, Locality, MayDefine, NodeId, OpId, PathId, Step, Stop};
use crate::engine::Engine;
use crate::node::{Node, Product, ReadKind, RunRecord};
use crate::order::{Pos, SegId};
use crate::sched::Worker;
use crate::values::{Field, ValueId, Values};

/// A handle on a value read through a path, extended step by step
/// (`Ctx::at`) and read at a step (`Ctx::get`): the run depends only on
/// what it took.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct View(u32);

pub(crate) struct ViewRec {
    pub(crate) kind: ReadKind,
    pub(crate) path: PathId,
}

pub struct Ctx<'a> {
    pub(crate) eng: &'a Engine,
    pub(crate) worker: Option<&'a Worker>,
    pub(crate) me: &'a Arc<Node>,
    /// Its segment and label at the run's start.
    pub(crate) seg: SegId,
    pub(crate) pos: Pos,
    pub(crate) product: Product,
    pub(crate) views: Vec<ViewRec>,
    /// Its last record, for `placed_output`.
    pub(crate) last: Option<Arc<RunRecord>>,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(eng: &'a Engine, worker: Option<&'a Worker>, me: &'a Arc<Node>) -> Ctx<'a> {
        todo!()
    }

    pub fn values(&self) -> &'a Values {
        &self.eng.values
    }

    pub fn node(&self) -> NodeId {
        self.me.id
    }

    pub fn op(&self) -> OpId {
        self.me.op
    }

    pub fn args(&self) -> &[ValueId] {
        &self.me.args
    }

    /// `Err(Stop)` once a commit has refuted a read of this run.
    fn check(&self) -> Result<(), Stop> {
        todo!()
    }

    /// The definition of `f` reaching me, or the environment's value.
    pub fn read(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        todo!()
    }

    /// The definition of `f` reaching another position.
    pub fn read_at(&mut self, f: Field, at: At) -> Result<FieldAt, Stop> {
        todo!()
    }

    /// `read` through a path: the run depends only on the path's answer.
    pub fn read_path(&mut self, f: Field, p: PathId) -> Result<Option<ValueId>, Stop> {
        todo!()
    }

    /// The node whose definition of `f` reaches me (None: the environment).
    pub fn def_site(&mut self, f: Field) -> Result<Option<NodeId>, Stop> {
        todo!()
    }

    /// The environment's value of `f`, whatever is defined before me.
    pub fn env(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        todo!()
    }

    pub fn define(&mut self, f: Field, x: ValueId) {
        self.define_with(f, x, Locality::Local);
    }

    /// `Locality::Scope` and `ScopeAt` are `unimplemented!("scopes: next run")`.
    pub fn define_with(&mut self, f: Field, x: ValueId, loc: Locality) {
        todo!()
    }

    /// `f` := the integer definition of `f` reaching me, plus `c`. Kept as
    /// a relation: when its input changes the engine recomputes it without
    /// running me, and once this operation has made one, its pending nodes
    /// are given it ahead of their runs. This is how a counter chain is not
    /// a chain of runs. The run need not read `f` first.
    pub fn define_rel(&mut self, f: Field, c: i64) {
        todo!()
    }

    /// Adds `x` to `f`'s contributions at my position.
    pub fn contribute(&mut self, f: Field, x: ValueId) {
        todo!()
    }

    /// The declared monoid's fold over the contributions to `f` after
    /// `from` (None: the unit's start) and before `to` (None: me).
    pub fn fold(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<ValueId, Stop> {
        todo!()
    }

    /// `fold` through a path.
    pub fn fold_path(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>, p: PathId) -> Result<Option<ValueId>, Stop> {
        todo!()
    }

    /// The first contribution to `f` after `after` (None: the unit's start).
    pub fn next_contribution(&mut self, f: Field, after: Option<NodeId>) -> Result<Option<(NodeId, ValueId)>, Stop> {
        todo!()
    }

    /// A view on `fold(f, from, to)`, read step by step.
    pub fn view(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<View, Stop> {
        todo!()
    }

    /// A view on `read(f)`.
    pub fn view_field(&mut self, f: Field) -> Result<View, Stop> {
        todo!()
    }

    /// The view one step further; None where the step does not apply.
    pub fn at(&mut self, v: View, s: Step) -> Option<View> {
        todo!()
    }

    /// The value at one step further, recorded as a read of that path.
    pub fn get(&mut self, v: View, s: Step) -> Option<ValueId> {
        todo!()
    }

    pub fn len(&mut self, v: View) -> Option<u64> {
        todo!()
    }

    pub fn has(&mut self, v: View, k: ValueId) -> bool {
        todo!()
    }

    /// The whole value under the view, recorded as a read of all of it.
    pub fn whole(&mut self, v: View) -> ValueId {
        todo!()
    }

    /// Places a node after me, before my continuation and next sibling;
    /// its index among my placements.
    pub fn child(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        todo!()
    }

    /// Places a node after me and my children.
    pub fn follow(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        todo!()
    }

    /// Places a node after `at` and its placements.
    pub fn follow_after(&mut self, at: NodeId, op: OpId, args: Vec<ValueId>) -> usize {
        todo!()
    }

    /// The output my `i`-th placement had at my last commit, if it has run.
    pub fn placed_output(&mut self, i: usize) -> Option<ValueId> {
        todo!()
    }

    /// `op` on `args` inline, memoised by (op, args): the output. It reads
    /// at my position and its reads are mine.
    pub fn apply(&mut self, op: OpId, args: Vec<ValueId>) -> Result<ValueId, Stop> {
        todo!()
    }

    /// Takes my next `n` siblings: they become inert and `following` gives
    /// their (op, args). Pushed siblings only, not placements.
    pub fn claim(&mut self, n: usize) {
        todo!()
    }

    /// The `i`-th claimed sibling's (op, args).
    pub fn following(&mut self, i: usize) -> Option<(OpId, Vec<ValueId>)> {
        todo!()
    }

    /// Narrows what I may define, for prediction. Accepted, not used by
    /// this core.
    pub fn narrow(&mut self, m: MayDefine) {}

    /// `Effect::None` is accepted; any other is `unimplemented!("scopes: next run")`.
    pub fn set_effect(&mut self, e: Effect) {
        todo!()
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
        todo!()
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
