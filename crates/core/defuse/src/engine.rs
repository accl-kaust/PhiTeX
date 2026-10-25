//! The engine as the client holds it: units, nodes, the environment,
//! evaluation, edits, reports.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::api::{
    CapPolicy, Cause, EditSource, EnvProvider, EvalError, Limits, LoopStart, Monoid, NodeId, NodeInfo, OpId,
    Operation, PathId, ProjId, Projection, Report, Settled, Step, UnitId,
};
use crate::index::FieldIndex;
use crate::lock::{Lock, Mx};
use crate::node::Node;
use crate::order::{Order, Pos};
use crate::segment::Segment;
use crate::table::Table;
use crate::values::{Field, FxHashMap, FxHashSet, ValueId, Values};

/// A unit's range of the line.
pub(crate) struct Unit {
    pub(crate) id: UnitId,
    pub(crate) start: Pos,
    pub(crate) end: Pos,
    /// The label of the last node pushed, for the next push.
    pub(crate) last: AtomicU64,
}

/// The environment: the values reaching the start of the line, from the
/// client's settings and, on a miss, its provider.
pub(crate) struct Env {
    pub(crate) values: Lock<FxHashMap<Field, Option<ValueId>>>,
    pub(crate) provider: Lock<Option<Box<EnvProvider>>>,
}

pub(crate) struct Paths {
    pub(crate) by_steps: Lock<FxHashMap<Box<[Step]>, PathId>>,
    pub(crate) steps: Table<Box<[Step]>>,
}

pub(crate) struct Monoids {
    pub(crate) by_field: FxHashMap<Field, Arc<dyn Monoid>>,
    pub(crate) by_prefix: Vec<(String, Arc<dyn Monoid>)>,
}

/// Counts an evaluation keeps outside the workers.
#[derive(Default)]
pub(crate) struct Stats {
    pub(crate) nodes_made: AtomicU64,
    pub(crate) edits: AtomicU64,
}

/// Every method takes `&self`: a mutation is a placement or a mark under a
/// segment's lock, and an edit during an evaluation is just more dirty
/// nodes. Only `evaluate` is exclusive by convention of the client.
pub struct Engine {
    pub(crate) values: Values,
    pub(crate) ops: Table<Arc<dyn Operation>>,
    pub(crate) units: Table<Unit>,
    pub(crate) nodes: Table<Arc<Node>>,
    pub(crate) segs: Table<Arc<Segment>>,
    pub(crate) order: Order,
    /// Segments defining each field; segments contributing to each field.
    pub(crate) defs: FieldIndex,
    pub(crate) contribs: FieldIndex,
    pub(crate) monoids: Lock<Monoids>,
    pub(crate) env: Env,
    pub(crate) paths: Paths,
    pub(crate) projections: Table<Projection>,
    /// Relations each operation has made: (op, field) -> c (`commit::learn_shape`).
    pub(crate) shapes: Lock<FxHashMap<(OpId, Field), i64>>,
    /// `Ctx::apply`'s memo.
    pub(crate) memo: Lock<FxHashMap<(OpId, Box<[ValueId]>), ValueId>>,
    /// Fields the client says never change after setup: reads of them are
    /// not recorded.
    pub(crate) stable: Lock<FxHashSet<Field>>,
    pub(crate) marked: Lock<FxHashSet<NodeId>>,
    pub(crate) settled: Mx<Vec<Settled>>,
    pub(crate) limits: Lock<Limits>,
    pub(crate) version: AtomicU64,
    pub(crate) stats: Stats,
    /// The last evaluation's report, for the chain queries.
    pub(crate) last_report: Mx<Option<Report>>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    pub fn new() -> Engine {
        todo!()
    }

    pub fn values(&self) -> &Values {
        &self.values
    }

    pub fn register(&self, op: Arc<dyn Operation>) -> OpId {
        todo!()
    }

    pub fn register_projection(&self, p: Projection) -> ProjId {
        todo!()
    }

    /// A path interned; the same steps give the same id.
    pub fn path(&self, steps: &[Step]) -> PathId {
        todo!()
    }

    /// A unit after the last one made.
    pub fn new_unit(&self) -> UnitId {
        todo!()
    }

    /// A unit in the gap after `node` (and its placements).
    pub fn new_unit_after(&self, node: NodeId) -> UnitId {
        todo!()
    }

    /// A node at the end of `unit`, dirty.
    pub fn push(&self, unit: UnitId, op: OpId, args: Vec<ValueId>) -> NodeId {
        todo!()
    }

    pub fn push_many(&self, unit: UnitId, op: OpId, args: Vec<Vec<ValueId>>) -> Vec<NodeId> {
        todo!()
    }

    /// A node after `node` (and its placements), dirty.
    pub fn insert_after(&self, node: NodeId, op: OpId, args: Vec<ValueId>) -> NodeId {
        todo!()
    }

    /// A node before `node`, dirty.
    pub fn insert_before(&self, node: NodeId, op: OpId, args: Vec<ValueId>) -> NodeId {
        todo!()
    }

    /// The node's text changed: same id, same position, new (op, args),
    /// dirty; what its old run defined is re-asked at its next commit.
    pub fn replace(&self, node: NodeId, op: OpId, args: Vec<ValueId>) {
        todo!()
    }

    /// Removes a node and its placements (`commit::unfile`).
    pub fn remove(&self, node: NodeId) {
        todo!()
    }

    /// Marks every node whose (op, args) passes `pred` dirty.
    pub fn refresh_where(&self, pred: &dyn Fn(OpId, &[ValueId], &Values) -> bool) {
        todo!()
    }

    pub fn output(&self, n: NodeId) -> Option<ValueId> {
        todo!()
    }

    /// Its placements in order.
    pub fn children(&self, n: NodeId) -> Vec<NodeId> {
        todo!()
    }

    pub fn claimed(&self, n: NodeId) -> Vec<NodeId> {
        todo!()
    }

    /// The node whose definition of `f` reaches `n` (None: the environment).
    pub fn reaching_def(&self, n: NodeId, f: Field) -> Option<NodeId> {
        todo!()
    }

    /// `n`'s own definition of `f`.
    pub fn def_value(&self, n: NodeId, f: Field) -> Option<ValueId> {
        todo!()
    }

    pub fn defs_of(&self, n: NodeId) -> Vec<(Field, ValueId)> {
        todo!()
    }

    /// The fold over all of `unit`'s contributions to `f`.
    pub fn acc_value(&self, unit: UnitId, f: Field) -> Option<ValueId> {
        todo!()
    }

    /// The fold of contributions to `f` (default: `SeqMonoid`).
    pub fn declare_accumulator(&self, f: Field, m: Arc<dyn Monoid>) {
        todo!()
    }

    pub fn declare_accumulator_prefix(&self, prefix: &str, m: Arc<dyn Monoid>) {
        todo!()
    }

    /// A field that never changes after setup: reads of it go unrecorded.
    pub fn set_stable(&self, f: Field) {
        todo!()
    }

    pub fn set_limits(&self, l: Limits) {
        *self.limits.write() = l;
    }

    pub fn limits(&self) -> Limits {
        *self.limits.read()
    }

    /// The version edits are reported at (`Settled::version`).
    pub fn set_version(&self, v: u64) {
        self.version.store(v, Ordering::Release);
    }

    pub fn set_environment(&self, env: impl IntoIterator<Item = (Field, ValueId)>) {
        todo!()
    }

    /// Changes environment values; readers of each changed field whose
    /// read reaches the environment are re-asked.
    pub fn update_environment(&self, changes: &[(Field, Option<ValueId>)]) {
        todo!()
    }

    pub fn environment(&self) -> Vec<(Field, ValueId)> {
        todo!()
    }

    /// Answers environment misses.
    pub fn set_env_provider(&self, provider: Box<EnvProvider>) {
        todo!()
    }

    pub fn clear_env_provider(&self) {
        todo!()
    }

    /// Runs on `threads` workers until nothing is dirty or running.
    pub fn evaluate(&self, threads: usize) -> Result<Report, EvalError> {
        todo!()
    }

    pub fn evaluate_sequential(&self) -> Result<Report, EvalError> {
        self.evaluate(1)
    }

    /// `evaluate`, then `src.absorb` while it gives edits; one report for
    /// all of it.
    pub fn evaluate_absorbing(&self, threads: usize, src: &mut dyn EditSource) -> Result<Report, EvalError> {
        todo!()
    }

    pub fn is_live(&self, node: NodeId) -> bool {
        todo!()
    }

    pub fn live_nodes(&self) -> usize {
        todo!()
    }

    /// The unit's nodes in position order.
    pub fn order(&self, u: UnitId) -> Vec<NodeId> {
        todo!()
    }

    pub fn node_info(&self, n: NodeId) -> Option<NodeInfo> {
        todo!()
    }

    /// Marks a node for `take_settled`.
    pub fn mark(&self, n: NodeId) {
        todo!()
    }

    pub fn unmark(&self, n: NodeId) {
        todo!()
    }

    /// Marked nodes whose output settled since the last take.
    pub fn take_settled(&self) -> Vec<Settled> {
        todo!()
    }

    pub fn debug_reads(&self, n: NodeId) -> String {
        todo!()
    }

    pub fn debug_node(&self, n: NodeId) -> String {
        todo!()
    }

    /// Checks every recorded read against its current answer and every
    /// relation against its input: a line per violation. Empty after any
    /// evaluation, or the engine is wrong.
    pub fn check_invariants(&self) -> Vec<String> {
        todo!()
    }

    /// The realised chain of the last evaluation: from the last node to
    /// commit, each woken by the one before, with its cause.
    pub fn critical_chain(&self) -> Vec<(NodeId, Cause)> {
        todo!()
    }

    /// The chain with each node's [run, commit, re-ask, gap-before] in ns.
    pub fn critical_chain_times(&self) -> Vec<(NodeId, [u64; 4])> {
        todo!()
    }

    /// The chain with each node's height: how many woke it in turn.
    pub fn critical_chain_heights(&self) -> Vec<(NodeId, u32, Cause)> {
        todo!()
    }

    /// The chain as (node, run ns, op name).
    pub fn realised_chain(&self) -> Vec<(NodeId, u64, String)> {
        todo!()
    }

    /// Accepted, unused: no run audit in this core.
    pub fn set_run_audit(&self, _on: bool) {}

    pub fn set_loop_start(&self, _s: LoopStart) {}

    pub fn set_max_rounds(&self, _n: u64) {}

    pub fn set_round_cap_policy(&self, _p: CapPolicy) {}

    pub fn declare_loopback(&self, _f: Field, _init: ValueId) {
        unimplemented!("loop-back: next run")
    }

    pub fn declare_loopback_early(&self, _f: Field, _init: ValueId) {
        unimplemented!("loop-back: next run")
    }

    pub fn declare_loopbacks(&self, _fields: &[(Field, ValueId)]) {
        unimplemented!("loop-back: next run")
    }

    pub fn loop_value(&self, _f: Field) -> Option<ValueId> {
        unimplemented!("loop-back: next run")
    }
}
