//! The public types clients program the engine with: operations and their
//! static hooks, effects, claims, reports, errors, schedulers, executors and
//! edit sources.

use std::sync::{Arc, Mutex};

use crate::Ctx;
use crate::values::{Field, Rng, ValueId, Values};

/// A registered operation.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct OpId(pub(crate) u32);

/// A node: a run of an operation on arguments at a place in a unit.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct NodeId(pub(crate) u64);

impl NodeId {
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn from_raw(raw: u64) -> NodeId {
        NodeId(raw)
    }
}

/// A unit: an ordered text of nodes with its own scopes.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct UnitId(pub(crate) u32);

/// Where a definition lives: the enclosing scope, or everywhere after it.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub enum Locality {
    Local,
    Global,
    /// A local of the scope the node opens after itself (`Ctx::define_in_scope`):
    /// seen inside that scope, not after it closes.
    Scope,
    /// A local of the `k`-th frame the node opens (0: the outermost;
    /// `Ctx::define_in_scope_at`): seen inside that frame and the frames
    /// within it, not after it closes.
    ScopeAt(u8),
}

impl Locality {
    /// The entry a definition of this locality is filed as (0: the node's
    /// own level; one per opened frame otherwise).
    #[must_use]
    pub(crate) fn entry_key(self) -> u8 {
        match self {
            Locality::Local | Locality::Global => 0,
            Locality::Scope => u8::MAX,
            Locality::ScopeAt(k) => k.saturating_add(1).min(u8::MAX - 1),
        }
    }
}

/// What a node with a pending definition set may define.
#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub enum MayDefine {
    Nothing,
    Any,
    Names(Vec<Field>),
    /// Fields whose names start with one of these.
    Prefixes(Vec<String>),
}

/// Scope brackets: `closes` scopes, then `opens` new ones, before or after
/// the node itself.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct Brackets {
    pub closes: u8,
    pub opens: u8,
    pub before: bool,
    pub kind: Option<ValueId>,
    /// The kind of the opened scope is set by the run (`Ctx::set_kind`).
    pub kind_pending: bool,
}

/// A node's scope effect.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub enum Effect {
    None,
    /// Opens a scope after itself.
    Open,
    /// Closes the scope it is in, after itself.
    Close,
    Scope(Brackets),
    /// Known only when the node runs (`Ctx::set_effect`).
    Pending,
}

impl Effect {
    /// As brackets (`Pending` as none).
    #[must_use]
    pub(crate) fn brackets(&self) -> Brackets {
        match self {
            Effect::None | Effect::Pending => Brackets::default(),
            Effect::Open => Brackets {
                opens: 1,
                ..Brackets::default()
            },
            Effect::Close => Brackets {
                closes: 1,
                ..Brackets::default()
            },
            Effect::Scope(b) => *b,
        }
    }
}

/// How many following siblings a node claims (they do not run; the claimer
/// reads their text).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub enum Claim {
    None,
    Count(usize),
    /// Decided by the run (`Ctx::claim`): the siblings it may cover wait.
    Pending,
}

/// Whether a stream claim ends before a candidate node.
#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub enum Until {
    No,
    Yes,
    /// Ends if the field's value at the candidate passes `until_value`.
    Read(Field),
    /// Ends if the fields' values at the candidate pass `until_values`.
    ReadEach(Vec<Field>),
}

/// Returned (via `?`) when a value is not available yet; the engine re-runs
/// the operation from the start once it is.
#[derive(Debug)]
pub struct Stop {
    pub(crate) _private: (),
}

/// An operation: what a node runs, and what can be known of it from its
/// arguments alone.
pub trait Operation: Send + Sync {
    fn name(&self) -> &'static str {
        "op"
    }

    /// The fields the node defines, if known from its arguments; `None`: a
    /// pending definition set, bounded by `may_define`.
    fn defs(&self, args: &[ValueId], values: &Values) -> Option<Vec<(Field, Locality)>>;

    fn may_define(&self, _args: &[ValueId], _values: &Values) -> MayDefine {
        MayDefine::Any
    }

    /// A further filter on `may_define`.
    fn may_define_field(&self, _args: &[ValueId], _values: &Values, _f: Field) -> bool {
        true
    }

    /// The fields the run reads (a hint).
    fn reads(&self, _args: &[ValueId], _values: &Values) -> Option<Vec<Field>> {
        None
    }

    /// The node's contributions to accumulators, if known from its text,
    /// per field: range readers of a listed field fold the listed value
    /// without waiting for the node's run, which must contribute exactly
    /// that: a run that differs is applied like any change to a
    /// contribution (the readers whose range holds it run again). A field
    /// not listed is not known: its range readers wait for the run, unless
    /// [`Operation::contribs_complete`] says the list is all of them (an
    /// empty list always is). A node claimed by another contributes
    /// nothing (its claimer does, in its place); a new text asks again.
    fn static_contribs(&self, _args: &[ValueId], _values: &Values) -> Option<Vec<(Field, ValueId)>> {
        None
    }

    /// Whether [`Operation::static_contribs`] lists all the node's
    /// contributions: a field not listed gets nothing from its run, so
    /// its range readers pass the node while it is in work (a run that
    /// contributes to one anyway is applied like any change).
    fn contribs_complete(&self, _args: &[ValueId], _values: &Values) -> bool {
        false
    }

    /// Whether the node runs only in the evaluation's final phase: held,
    /// unrun, while loop-back fields still move, and run once the rounds
    /// have converged (`next_round` found nothing changed), after which the
    /// evaluation ends. Its readers wait for it as for any node in work. A
    /// final node whose publication moves a loop-back field fails the
    /// evaluation ([`EvalError::FinalMovedLoop`]). An evaluation without
    /// rounds runs it once at its end; an edited final node runs again only
    /// in the final phase. Asked once per text.
    fn final_only(&self, _args: &[ValueId], _values: &Values) -> bool {
        false
    }

    /// The definitions this text is predicted to make, with their values,
    /// before it runs (for example from an earlier run on the same
    /// inputs). Under any speculation but [`crate::Speculation::Off`], a
    /// reader may run on a prediction instead of waiting for the node.
    /// When the node runs, each predicted field is compared by value with
    /// what it defined: equal, the readers stand; different (or predicted
    /// and not defined), exactly the readers of that field run again.
    /// Predictions change speed, never results. Asked once per text: a
    /// new text asks again.
    fn predicted_defs(&self, _args: &[ValueId], _values: &Values) -> Option<Vec<(Field, ValueId)>> {
        None
    }

    fn effect(&self, _args: &[ValueId], _values: &Values) -> Effect {
        Effect::None
    }

    fn claim(
        &self,
        _args: &[ValueId],
        _values: &Values,
        _following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        Claim::None
    }

    /// How far a pending claim may reach.
    fn claim_limit(
        &self,
        _args: &[ValueId],
        _values: &Values,
        _following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Option<usize> {
        None
    }

    fn until(&self, _args: &[ValueId], _values: &Values, _cand: OpId, _cand_args: &[ValueId]) -> Until {
        Until::No
    }

    fn until_value(
        &self,
        _args: &[ValueId],
        _values: &Values,
        _cand: OpId,
        _cand_args: &[ValueId],
        value: Option<ValueId>,
    ) -> bool {
        value.is_some()
    }

    fn until_values(
        &self,
        _args: &[ValueId],
        _values: &Values,
        _cand: OpId,
        _cand_args: &[ValueId],
        fields: &[Option<ValueId>],
    ) -> bool {
        fields.iter().all(Option::is_some)
    }

    /// A predicted length of a stream claim (a speculation hint).
    fn until_predict(
        &self,
        _args: &[ValueId],
        _values: &Values,
        _following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Option<usize> {
        None
    }

    /// The run.
    ///
    /// # Errors
    ///
    /// `Stop` when a read's subject is still in work.
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop>;
}

/// A monoid folding accumulator contributions.
pub trait Monoid: Send + Sync {
    fn empty(&self, values: &Values) -> ValueId;
    fn combine(&self, values: &Values, a: ValueId, b: ValueId) -> ValueId;

    /// Whether a range read of this accumulator, when the scheduler
    /// predicts, folds the contributors still in work as they last
    /// contributed (nothing, if they never ran) instead of waiting for
    /// them. A contributor that then contributes otherwise re-asks the
    /// reader, which runs again on the committed values only. For
    /// accumulators whose contributions do not depend on what their
    /// contributors read (counter writes: set or add a constant), so a
    /// cold build settles in two waves; a guess that is mostly wrong
    /// costs a run per reader.
    fn speculate(&self) -> bool {
        false
    }
}

/// Answers an environment read the engine does not hold
/// ([`Engine::set_env_provider`]), with the value table to name the field
/// (`values.get(field.0)` is its name) and to intern the answer.
pub type EnvProvider = dyn Fn(&Values, Field) -> Option<Option<ValueId>> + Send + Sync;

/// Sequence concatenation (structurally shared, see [`Values::seq_concat`]).
pub struct SeqMonoid;

impl Monoid for SeqMonoid {
    fn empty(&self, _: &Values) -> ValueId {
        ValueId::NIL
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        v.seq_concat(a, b)
    }
}

/// Integer sum.
pub struct SumMonoid;

impl Monoid for SumMonoid {
    fn empty(&self, v: &Values) -> ValueId {
        v.int(0)
    }

    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        v.int(v.as_int(a).unwrap_or(0).wrapping_add(v.as_int(b).unwrap_or(0)))
    }
}

/// Why a node had to run again (for [`EvalError::RunawayNode`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cause {
    /// A use of `field` now reaches another definition.
    Rebind {
        field: Field,
        from: Option<NodeId>,
        to: Option<NodeId>,
    },
    /// The definition of `field` at `site` (the unit's start: `None`)
    /// changed its value.
    Changed { field: Field, site: Option<NodeId> },
    /// The output of `of`, which it reads, changed.
    Output { of: NodeId },
    /// The node it waited on finished (or went away): for `field` (`None`
    /// for its output or claim), and when the wait was on a pending
    /// definition set declared by prefixes, the prefix the field matched.
    Woken {
        by: NodeId,
        field: Option<Field>,
        prefix: Option<String>,
    },
    /// A loop-back field's value for the next round.
    Loop { field: Field },
    /// The environment's `field` changed.
    Environment { field: Field },
    /// The structure around it changed (a claim, a scope, text coming or
    /// going, a range it reads).
    Structure,
    /// An accumulator range it reads over `field`, settled last by `from`
    /// (for [`Engine::critical_chain`]).
    Range { field: Field, from: Option<NodeId> },
    /// Its creation: unfolded by `by` (a parent, or the node whose run
    /// made it a continuation; `None` for a pushed node), starting above
    /// `by` and above the site of the hinted read `from_hint` (the one
    /// that stood highest), for [`Engine::critical_chain`].
    Created {
        by: Option<NodeId>,
        from_hint: Option<(Field, NodeId)>,
    },
    /// It was re-done at `to`, the level above `by`: a finished node it
    /// needed that was not settled below its level, when nothing it read
    /// or waited for stands at the level below (for
    /// [`Engine::critical_chain`], which goes on from `by`; `None`: gone).
    Raised { to: u32, by: Option<NodeId>, how: Lifted },
}

/// What [`crate::Ctx::use_field_at`] found at a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub enum FieldAt {
    /// The definition reaching it: its value and its defining node
    /// (`None` for the unit's start value).
    Def(ValueId, Option<NodeId>),
    /// No definition reaches it.
    Unset,
    /// The position was removed or re-made: the reader re-runs when it
    /// goes, and whatever names the position gives it the new node.
    Gone,
}

/// A projection of a field's value, registered with
/// [`crate::Engine::register_projection`] and read with
/// [`crate::Ctx::use_field_proj`]: a reader depends only on the projected
/// value, so a change that projects equal leaves it standing.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct ProjId(pub(crate) u16);

impl ProjId {
    /// `true` for an empty, zero, nil or unit value, `false` otherwise
    /// (an absent definition stays absent).
    pub const IS_EMPTY: ProjId = ProjId(1);
}

/// A client projection: pure, from a value to a small value.
pub type Projection = Arc<dyn Fn(ValueId, &Values) -> ValueId + Send + Sync>;

/// Why a node was last made to run again ([`Report::rerun_causes`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rerun {
    /// Its text was replaced.
    Text,
    /// The definition of `field` it read changed value (`old` is
    /// [`ValueId::UNIT`] when the field had none).
    Changed { field: Field, old: ValueId, new: ValueId },
    /// A use of `field` now reaches another definition.
    Rebind { field: Field },
    /// A speculative run's assumption on `field` failed (`None`: on an
    /// output, or a read that a node of its own batch had re-bound).
    Refuted { field: Option<Field> },
    /// It stopped, and what it waited for finished (`field`: `None` for
    /// an output or a claim).
    Woken { field: Option<Field> },
    /// A loop-back field's value for the next round.
    Loop { field: Field },
    /// The environment's `field` changed.
    Environment { field: Field },
    /// The output of `of`, which it reads, changed or went.
    Output { of: NodeId },
    /// Its claim (or a claim stream's following siblings) changed: a
    /// sibling came, went, changed or was claimed by another.
    Claim,
    /// The anchor `of` it placed after (`follow_after`) or read at
    /// (`use_field_at`) came or went.
    Anchor { of: NodeId },
    /// A sibling its `prev_sibling_where` scan passed over changed.
    SiblingScan,
    /// An accumulator range it reads changed (a contribution or the
    /// range's structure).
    Range { field: Option<Field> },
    /// A scope effect around it changed.
    Scope,
    /// Anything else.
    Other,
}

/// How a finished node put another above it ([`Cause::Raised`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifted {
    /// Its run read the node (a value, an output, a range) above its
    /// level and was re-done above it.
    Read,
    /// Its gate found the node, the definer of a hinted field, finished
    /// at or above its level before the run started.
    Gate,
}

/// What a waiting node waits on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitOn {
    /// Parked until that node finishes (its definitions, claim, scope
    /// effect or output), for the field (`None`: its output or claim).
    Node(NodeId, Option<Field>),
    /// Parked until every node in its accumulator range has finished.
    Range { field: Field },
    /// Parked until the client supplies that environment field.
    Environment(Field),
    /// Not parked: dirty and queued, waiting for its turn.
    Queued,
}

/// A node still in work when an evaluation ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiter {
    pub node: NodeId,
    pub on: WaitOn,
}

/// What a stopped run waits for (the breakdown in
/// [`Report::waits_by_kind`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitKind {
    /// A definition it reads is being computed.
    Value,
    /// A node between the definition and the read may define the field
    /// (its definition set is pending).
    Defs,
    /// A node between them may open or close a scope.
    Effect,
    /// A pending claim before the definition may swallow it.
    Claim,
    /// A child it needs is being computed.
    Child,
    /// An accumulator range it reads has members in work.
    Range,
    /// An environment field not yet supplied.
    Environment,
}

impl WaitKind {
    /// The kind's name in the report.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            WaitKind::Value => "value",
            WaitKind::Defs => "definition set",
            WaitKind::Effect => "scope effect",
            WaitKind::Claim => "claim",
            WaitKind::Child => "child",
            WaitKind::Range => "range",
            WaitKind::Environment => "environment",
        }
    }
}

/// What reaching [`crate::Engine::set_max_rounds`]'s cap does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub enum CapPolicy {
    /// The evaluation fails with [`EvalError::NonConvergence`].
    #[default]
    Error,
    /// The rounds stop: the final phase ([`Operation::final_only`]) runs
    /// on the loop-back values of the last round run, and the evaluation
    /// ends with the fields still moving in [`Report::capped`].
    Finalize,
}

/// Why an evaluation ended without a result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvalError {
    Cycle(Vec<NodeId>),
    NonConvergence {
        rounds: u64,
    },
    ConflictingPublication {
        field: Field,
    },
    UndeclaredDefinition {
        node: NodeId,
        field: Field,
    },
    /// A node ran more than [`Limits::runs_per_node`] times in one
    /// evaluation: its run count, and why its last runs were needed
    /// (oldest first).
    RunawayNode {
        node: NodeId,
        runs: u32,
        causes: Vec<Cause>,
    },
    /// A node created more than [`Limits::children_per_node`] children
    /// in one evaluation, or unfolded deeper than [`Limits::unfold_depth`]:
    /// the count, the depth, and the last children (operation name,
    /// arguments) it made.
    RunawayCreation {
        node: NodeId,
        created: u32,
        depth: u32,
        last: Vec<(String, String)>,
    },
    /// [`Limits::idle_rounds`] rounds in a row applied no result while
    /// these nodes waited.
    NoProgress {
        rounds: u32,
        waiters: Vec<Waiter>,
        /// The evaluation's counters up to here.
        report: Box<Report>,
    },
    /// [`crate::Ctx::use_field_at`] named the reader itself or a node of
    /// another unit.
    BadPosition {
        node: NodeId,
        at: NodeId,
    },
    /// [`Limits::total_runs`] or [`Limits::time`] was reached with these
    /// nodes still waiting or queued.
    /// An operation's run panicked: the node, its operation's name and
    /// the panic message. The node runs again in the next evaluation.
    OpPanicked {
        node: NodeId,
        op: String,
        message: String,
    },
    /// One of an operation's static hooks (`defs`, `may_define`,
    /// `may_define_field`, `effect`, `claim`, `claim_limit`, `reads`,
    /// `static_contribs`, `predicted_defs`, `until`, `until_value`,
    /// `until_values`) panicked: the node
    /// when known (a text analysed before it is placed has none), the
    /// operation's name, the hook and the panic message. A panic outside
    /// an evaluation (in `push`) is reported by the next one.
    HookPanicked {
        node: Option<NodeId>,
        op: String,
        hook: &'static str,
        message: String,
    },
    /// A publication moved loop-back field `field` after the rounds had
    /// converged: by final node `node` ([`Operation::final_only`]), or by
    /// a node that ran in the final phase because it read one.
    FinalMovedLoop {
        node: NodeId,
        field: Field,
    },
    Deadline {
        runs: u64,
        waiters: Vec<Waiter>,
        /// The evaluation's counters up to here.
        report: Box<Report>,
    },
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::Cycle(ns) => write!(f, "cycle among {} nodes", ns.len()),
            EvalError::NonConvergence { rounds } => write!(f, "no fixpoint after {rounds} rounds"),
            EvalError::ConflictingPublication { field } => {
                write!(f, "conflicting publications for {field:?}")
            }
            EvalError::UndeclaredDefinition { node, field } => {
                write!(f, "node {node:?}: definition of {field:?} contradicts its declaration")
            }
            EvalError::RunawayNode { node, runs, causes } => {
                write!(f, "node {node:?} ran {runs} times; last causes: {causes:?}")
            }
            EvalError::RunawayCreation {
                node,
                created,
                depth,
                last,
            } => write!(
                f,
                "node {node:?} created {created} children at depth {depth}; last: {last:?}"
            ),
            EvalError::NoProgress { rounds, waiters, report } => write!(
                f,
                "{rounds} rounds without progress; {} nodes waiting: {waiters:?}; waited on: {:?}",
                waiters.len(),
                report.stuck_subjects
            ),
            EvalError::BadPosition { node, at } => {
                write!(
                    f,
                    "node {node:?} read a field at {at:?}, a position it may not read at"
                )
            }
            EvalError::OpPanicked { node, op, message } => {
                write!(f, "node {node:?} ({op}) panicked: {message}")
            }
            EvalError::HookPanicked {
                node,
                op,
                hook,
                message,
            } => {
                write!(f, "{op}::{hook} panicked (node {node:?}): {message}")
            }
            EvalError::FinalMovedLoop { node, field } => {
                write!(f, "final node {node:?} moved loop-back field {field:?}")
            }
            EvalError::Deadline { runs, waiters, .. } => write!(
                f,
                "deadline after {runs} runs; {} nodes waiting: {waiters:?}",
                waiters.len()
            ),
        }
    }
}

impl std::error::Error for EvalError {}

/// Bounds on one evaluation: exceeding one ends it with an error instead
/// of running away.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct Limits {
    /// Applied runs of one node (default 64).
    pub runs_per_node: u32,
    /// Children one node creates (default 4096).
    pub children_per_node: u32,
    /// Depth of unfolding, a child of a child and so on (default 256).
    pub unfold_depth: u32,
    /// Rounds in a row that apply no result (default 256).
    pub idle_rounds: u32,
    /// Applied runs in the whole evaluation (default none).
    pub total_runs: Option<u64>,
    /// Wall time, checked between rounds (default none; ignored on
    /// targets without a clock, such as wasm).
    pub time: Option<std::time::Duration>,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            runs_per_node: 64,
            children_per_node: 4096,
            unfold_depth: 256,
            idle_rounds: 256,
            total_runs: None,
            time: None,
        }
    }
}

/// Where an edit's loop-back iteration starts.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub enum LoopStart {
    /// The declared initial values (a cold evaluation's fixpoint).
    Initial,
    /// The last fixpoint.
    Previous,
    /// Every evaluation after edits runs the rounds from the declared
    /// initial values, as a fresh engine would (round for round, the cap
    /// and the final phase included); round k reuses what each node
    /// committed in the previous evaluation's round k whenever the reads
    /// it made there still hold (checked as a commit checks them).
    /// After value edits the edits are first evaluated from the last
    /// fixpoint (the quick path), kept when every round would see the
    /// same change: each node that committed, and each reader of a value
    /// that changed, committed the same in every round and read no
    /// loop-back that moved ([`Report::quick_held`],
    /// [`Report::quick_refused`]).
    PerRound,
}

/// A marked node reported final at a version.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Settled {
    pub node: NodeId,
    pub version: u64,
    pub output: ValueId,
}

/// A node as the client sees it ([`Engine::node_info`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo {
    /// Its operation's name.
    pub op: &'static str,
    pub args: Vec<ValueId>,
    pub unit: UnitId,
    /// The node whose run unfolded it, if any.
    pub parent: Option<NodeId>,
    pub state: NodeState,
}

/// A node's state, coarsely.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NodeState {
    /// To run (or being run) in the current or next evaluation.
    Dirty,
    /// Waiting for something.
    Parked,
    /// Ran, and nothing it read has changed since.
    Done,
    /// Taken by a claim: not run.
    Inert,
}

/// Where one loop-back round's time went (`Report::round_times`):
/// nanoseconds, summed over workers except `wall`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct RoundTimes {
    /// The round's elapsed time.
    pub wall_ns: u64,
    /// In operations' runs (the client's code, the reads it makes inside).
    pub op_ns: u64,
    /// In commits (applying a run's result to the store).
    pub commit_ns: u64,
    /// In checking committed logs again (validation without a run).
    pub check_ns: u64,
    /// Workers waiting with nothing ready.
    pub idle_ns: u64,
    /// Runs of operations in the round.
    pub runs: u64,
    /// Results reused from the previous evaluation's same round
    /// ([`LoopStart::PerRound`]).
    pub memo_hits: u64,
}

/// Where the realised critical chain's time went (ns, wall clock; zero on
/// wasm32), each link being a run that waited for the one below it: from
/// the one below committing to the run being queued (`held`: parked on a
/// wait, held for the final phase, a later round or the environment;
/// from the evaluation's start for the lowest), queued to starting
/// (`dispatch`), running (`op`, its committed run only), and from its end
/// to its commit (`wait_merge`). A run queued before the one below
/// committed counts as queued from then; one that started before (a
/// guess) has neither. Per run: [`crate::Engine::critical_chain_times`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct LinkTimes {
    pub held: u64,
    pub dispatch: u64,
    pub op: u64,
    pub wait_merge: u64,
    /// Links.
    pub links: u64,
}

/// The serial merge's time by phase (ns; zero on wasm32), over
/// [`Report::merged`] results. `total` is all of it; the phases are
/// parts of it, the rest (setting up and finishing builders, counting)
/// being `total` less their sum.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct MergeTimes {
    pub total: u64,
    /// Checking a result's reads against the world.
    pub validate: u64,
    /// A refuted result: its cause, queued again.
    pub refute: u64,
    /// Placing children and continuations.
    pub place: u64,
    /// Filing its reads (the reader indexes).
    pub reads: u64,
    /// Field writes: definitions (and the node's new result stored).
    pub defs: u64,
    /// Field writes: contributions, with the range readers over them.
    pub contribs: u64,
    /// Outputs and claims.
    pub outs: u64,
    /// Re-asking the readers its changes reached, and settling claims
    /// (the dependents).
    pub dependents: u64,
    /// Queueing the nodes made dirty and waking the woken.
    pub wake: u64,
    /// Heights for the critical chain.
    pub heights: u64,
    /// Results whose world was unchanged since their run (a count: their
    /// reads hold unchecked).
    pub unchanged: u64,
}

impl MergeTimes {
    pub(crate) fn add(&mut self, o: &Self) {
        self.total += o.total;
        self.validate += o.validate;
        self.refute += o.refute;
        self.place += o.place;
        self.reads += o.reads;
        self.defs += o.defs;
        self.contribs += o.contribs;
        self.outs += o.outs;
        self.dependents += o.dependents;
        self.wake += o.wake;
        self.heights += o.heights;
        self.unchanged += o.unchanged;
    }
}

/// What an evaluation did. The first seven counters (`nodes_run` ..
/// `restarts`) are canonical: functions of the texts before and after,
/// equal under every schedule.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Report {
    /// On [`EvalError::NoProgress`] and [`EvalError::Deadline`]: the
    /// scheduler's state when the evaluation gave up (the queue, the nodes
    /// in work and what each waits on; see [`crate::Engine::state_dump`]).
    pub state_dump: Option<String>,
    /// Nodes whose run inputs (text and read answers) changed.
    pub nodes_run: u64,
    /// Of those, nodes whose output did not change.
    pub cutoffs: u64,
    /// Nodes whose bindings (definition sites read) changed.
    pub rebinds: u64,
    /// Kept states' reads asked again before resuming (or checking a
    /// placer), and the reads those states held: a state asks only the
    /// reads a change since its last check may break.
    pub kept_asked: u64,
    pub kept_held: u64,
    pub created: u64,
    pub freed: u64,
    pub rounds: u64,
    pub restarts: u64,
    /// Complete runs of operations (a stopped attempt is not one: see
    /// `stops`; `runs_by_op` counts every attempt).
    pub runs: u64,
    pub runs_by_op: Vec<(&'static str, u64)>,
    /// Waits (a run or a validation parked on a subject in work).
    pub stops: u64,
    pub waits_by_op: Vec<(&'static str, u64)>,
    pub waits_by_kind: Vec<(&'static str, u64)>,
    pub top_waited: Vec<(NodeId, u64)>,
    /// Resumptions after a wait.
    pub raises: u64,
    pub pending_waits: u64,
    pub pending_sets: u64,
    pub pending_sets_defining: u64,
    pub gated: u64,
    pub speculations: u64,
    pub confirmations: u64,
    pub mispredictions: u64,
    pub fuel_outs: u64,
    pub spec_stops: u64,
    /// Per loop-back round, where the time went.
    pub round_times: Vec<RoundTimes>,
    /// Runs whose claim differed from the one in force (a static
    /// `Claim::None`/`Count` taken as a guess, or the run's last claim).
    pub claims_refuted: u64,
    /// Definitions a run made outside its node's static list (`defs`):
    /// the list widened, the field's later readers re-validated.
    pub defs_refuted: u64,
    /// Runs whose scope effect differed from the declared one.
    pub effects_refuted: u64,
    /// Siblings claimed or released by those claims.
    pub claim_siblings: u64,
    pub refuted_by_field: Vec<(Field, u64)>,
    pub refuted_nodes: u64,
    pub refutations_max: u32,
    pub resumed: u64,
    pub restarted: u64,
    pub restarted_by: Vec<(&'static str, u64)>,
    pub declarations_refuted: u64,
    /// Runs whose contributions differed from what their text declared
    /// ([`Operation::static_contribs`]): the run's stand, and the range
    /// readers that folded the declared ones run again.
    pub static_contribs_refuted: u64,
    /// Every node made to run again, with the reason that made it dirty
    /// (the last one when it was made dirty more than once; edits before
    /// the evaluation included).
    pub rerun_causes: Vec<(NodeId, Rerun)>,
    /// The longest chain of runs in this evaluation that had to wait for
    /// each other: a run counts 1 more than the deepest run of this
    /// evaluation whose result it read, not counting reads answered by a
    /// guess (a previous value or a prediction) that was confirmed. A run
    /// with no such read counts 1, so a plain chain of `n` runs gives `n`
    /// (where [`Self::critical_path`], heights from 0, gives `n - 1`).
    pub realised_critical_path: u32,
    /// [`Self::realised_critical_path`] by time: the longest chain of
    /// runs that waited for each other, by the sum of their run times
    /// (zero on wasm32).
    pub realised_critical_time: std::time::Duration,
    /// The realised chain's links by wall time ([`LinkTimes`]), summed
    /// over its runs.
    pub critical_links: LinkTimes,
    /// Results through the serial merge (committed, stopped or refuted).
    pub merged: u64,
    /// The serial merge's time (ns): checking, committing or refuting
    /// each result and queueing what it changed, by phase.
    pub merge: MergeTimes,
    /// Each node's run time in this evaluation (all its attempts), longest
    /// first (empty on wasm32).
    pub run_times: Vec<(NodeId, std::time::Duration)>,
    /// Speculative runs that read a predicted value
    /// ([`Operation::predicted_defs`]) and were confirmed.
    pub predictions_confirmed: u64,
    /// Speculative runs that read a predicted value and were refuted.
    pub predictions_refuted: u64,
    /// Nodes refuted this evaluation (a guess or prediction broke) that
    /// ran conservatively afterwards, on committed values only (not those
    /// refuted only on a definition whose definer ran again for a
    /// loop-back value that moved). High counts mean a hint or prediction
    /// is costing parallelism.
    pub nodes_conservative: u64,
    pub env_stalled: u64,
    pub env_waits: Vec<(NodeId, Field)>,
    pub env_provided: u64,
    pub steps: u64,
    pub applied_widths: Vec<u64>,
    /// The largest batch of nodes run in one step: the most that could
    /// run at once.
    pub max_ready: usize,
    pub critical_path: u32,
    /// The edits made so far, those absorbed during the evaluation included.
    pub edits: u64,
    /// The sum of the batch sizes over the steps.
    pub ready_total: u64,
    /// Speculative runs kept whose shape (children, claim, scope effect or
    /// definition set) differed from the node's previous shape.
    pub spec_shape_changes: u64,
    pub version: u64,
    pub edits_applied: Vec<(u64, u64)>,
    pub settled: u64,
    /// Final nodes ([`Operation::final_only`]) released and run in the
    /// final phase.
    pub final_nodes_run: u64,
    /// Under [`CapPolicy::Finalize`], when the round cap was reached: the
    /// loop-back fields that were still moving.
    pub capped: Option<Vec<Field>>,
    /// Under [`LoopStart::PerRound`]: the quick path held (the edits were
    /// evaluated from the last fixed point only).
    pub quick_held: bool,
    pub resettled: u64,
    /// At a `NoProgress` stop: each node waited on that is not itself
    /// waiting, and its state (why nothing wakes the waiters).
    pub stuck_subjects: Vec<String>,
}

impl Report {
    /// The count for operation `name` in a per-operation list.
    #[must_use]
    pub fn by_op(list: &[(&'static str, u64)], name: &str) -> u64 {
        list.iter().filter(|x| x.0 == name).map(|x| x.1).sum()
    }
}

/// One unit of work handed to an [`Executor`].
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Job {
    pub node: NodeId,
    /// May read predicted (not yet final) values, and change program shape
    /// provisionally.
    pub speculative: bool,
    pub(crate) slot: u32,
}

impl Job {
    #[must_use]
    pub fn new(node: NodeId, speculative: bool) -> Self {
        Job {
            node,
            speculative,
            slot: u32::MAX,
        }
    }
}

/// A node that could run speculatively.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Candidate {
    pub node: NodeId,
    pub(crate) slot: u32,
    /// It ran before: its previous bindings and structure are predictions.
    pub has_prediction: bool,
    /// It is waiting on a value or pending structure.
    pub waiting: bool,
}

/// Chooses what runs next and what is speculated on. Policy affects speed
/// only, never results.
pub trait Scheduler {
    /// Speculative candidates offered per step (0 disables speculation).
    fn speculation_budget(&self) -> usize {
        0
    }

    /// Global cap on outstanding unconfirmed speculative runs.
    fn max_outstanding(&self) -> usize {
        4096
    }

    /// Step limit for one speculative run.
    fn fuel(&self) -> u64 {
        10_000
    }

    /// Fill `batch` from `ready` (safe to run) and `candidates` (may run
    /// speculatively). At least one ready node always runs.
    fn select(&mut self, ready: &[NodeId], candidates: &[Candidate], batch: &mut Vec<Job>);
}

/// When to speculate.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Speculation {
    Off,
    /// Only nodes with a prediction from a previous run.
    Predicted,
    /// Any candidate.
    All,
}

/// Runs every ready node; speculates on up to `budget` candidates per step.
#[derive(Clone, Debug)]
pub struct Greedy {
    pub speculation: Speculation,
    pub budget: usize,
    pub outstanding: usize,
    pub fuel: u64,
}

impl Default for Greedy {
    fn default() -> Self {
        Greedy {
            speculation: Speculation::Off,
            budget: 0,
            outstanding: 4096,
            fuel: 10_000,
        }
    }
}

impl Greedy {
    #[must_use]
    pub fn speculative(speculation: Speculation, budget: usize) -> Self {
        Greedy {
            speculation,
            budget,
            ..Greedy::default()
        }
    }
}

impl Scheduler for Greedy {
    fn speculation_budget(&self) -> usize {
        if self.speculation == Speculation::Off {
            0
        } else {
            self.budget
        }
    }

    fn max_outstanding(&self) -> usize {
        self.outstanding
    }

    fn fuel(&self) -> u64 {
        self.fuel
    }

    fn select(&mut self, ready: &[NodeId], candidates: &[Candidate], batch: &mut Vec<Job>) {
        batch.extend(ready.iter().map(|&n| Job::new(n, false)));
        for c in candidates {
            if self.speculation == Speculation::All || c.has_prediction {
                batch.push(Job::new(c.node, true));
            }
        }
    }
}

/// Random subsets of ready nodes and candidates, random fuel: for checking
/// that results are schedule-independent.
#[derive(Clone, Debug)]
pub struct RandomScheduler {
    pub rng: Rng,
    pub speculate: bool,
    pub budget: usize,
}

impl RandomScheduler {
    #[must_use]
    pub fn new(seed: u64, speculate: bool) -> Self {
        RandomScheduler {
            rng: Rng::new(seed),
            speculate,
            budget: 16,
        }
    }
}

impl Scheduler for RandomScheduler {
    fn speculation_budget(&self) -> usize {
        if self.speculate { self.budget } else { 0 }
    }

    fn max_outstanding(&self) -> usize {
        64
    }

    fn fuel(&self) -> u64 {
        // Sometimes tiny, to exercise fuel exhaustion.
        if self.speculate {
            3 + (self.rng.state() % 64)
        } else {
            10_000
        }
    }

    fn select(&mut self, ready: &[NodeId], candidates: &[Candidate], batch: &mut Vec<Job>) {
        for &n in ready {
            if self.rng.chance(1, 2) {
                batch.push(Job::new(n, self.speculate && self.rng.chance(1, 4)));
            }
        }
        for c in candidates {
            if self.rng.chance(1, 2) {
                batch.push(Job::new(c.node, true));
            }
        }
        for i in (1..batch.len()).rev() {
            let j = self.rng.below(i + 1);
            batch.swap(i, j);
        }
    }
}

/// The opaque result of one job of an executor batch.
pub struct RunResult(pub(crate) ());

/// Runs the engine's jobs: the seam where a client puts its threads (the
/// library never spawns any).
pub trait Executor {
    /// Runs `run(i)` for `i < n`, appending the results in index order.
    fn execute(&mut self, n: usize, run: &(dyn Fn(usize) -> RunResult + Sync), out: &mut Vec<RunResult>);

    /// How many jobs it may run at once: 1 for one thread.
    fn parallelism(&self) -> usize {
        1
    }

    /// Whether an evaluation on one thread goes in steps (a scheduling
    /// policy with identical results): each step's batch is chosen by the
    /// [`Scheduler`], and its commits are seen by the next step. `false`
    /// (the default): the one worker runs free, in text order, each
    /// commit seen at once. [`Sequential`] steps.
    fn steps(&self) -> bool {
        false
    }

    /// Runs `f(i)` for `i < n`, possibly in parallel.
    fn for_each(&mut self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        let mut out = Vec::with_capacity(n);
        self.execute(
            n,
            &|i| {
                f(i);
                RunResult(())
            },
            &mut out,
        );
    }
}

/// Everything on the calling thread.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sequential;

impl Executor for Sequential {
    fn execute(&mut self, n: usize, run: &(dyn Fn(usize) -> RunResult + Sync), out: &mut Vec<RunResult>) {
        out.extend((0..n).map(run));
    }

    fn steps(&self) -> bool {
        true
    }
}

/// Where edits come from during an evaluation.
pub trait EditSource {
    /// Applies edits that arrived; returns the client's version tag if any.
    fn absorb(&mut self, eng: &mut crate::Engine) -> Option<u64>;

    /// Marked nodes reported final; `false` leaves them to
    /// `Engine::take_settled`.
    fn settled(&mut self, _eng: &crate::Engine, _items: &[Settled]) -> bool {
        false
    }
}

/// No edits.
pub struct NoEdits;

impl EditSource for NoEdits {
    fn absorb(&mut self, _eng: &mut crate::Engine) -> Option<u64> {
        None
    }
}

/// An edit a queue carries.
#[derive(Clone, Debug)]
pub enum Edit {
    /// [`Engine::replace`].
    Replace { node: NodeId, op: OpId, args: Vec<ValueId> },
    /// [`Engine::insert_after`]; the new node's id is reported back.
    InsertAfter {
        after: NodeId,
        op: OpId,
        args: Vec<ValueId>,
    },
    /// [`Engine::insert_before`]; the new node's id is reported back.
    InsertBefore {
        before: NodeId,
        op: OpId,
        args: Vec<ValueId>,
    },
    /// [`Engine::push`]; the new node's id is reported back.
    Push { unit: UnitId, op: OpId, args: Vec<ValueId> },
    /// [`Engine::remove`].
    Remove { node: NodeId },
    /// [`Engine::set_environment`].
    Environment { fields: Vec<(Field, ValueId)> },
    /// [`Engine::mark`] (`on`) or [`Engine::unmark`].
    Mark { node: NodeId, on: bool },
}

#[derive(Default)]
struct QueueState {
    pending: Vec<(Edit, u64)>,
    applied: usize,
    made: Vec<(u64, NodeId)>,
    settled: Vec<Settled>,
}

/// Edits queued from any thread, absorbed by an evaluation; carries the
/// final-node reports back.
#[derive(Clone, Default)]
pub struct EditQueue {
    st: Arc<Mutex<QueueState>>,
}

impl EditQueue {
    #[must_use]
    pub fn new() -> EditQueue {
        EditQueue::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queues `edit` under the client's version `tag`.
    pub fn push(&self, edit: Edit, tag: u64) {
        self.lock().pending.push((edit, tag));
    }

    #[must_use]
    pub fn pending(&self) -> usize {
        self.lock().pending.len()
    }

    #[must_use]
    pub fn applied(&self) -> u64 {
        self.lock().applied as u64
    }

    /// The nodes made by `Push` and insert edits, by tag.
    #[must_use]
    pub fn take_made(&self) -> Vec<(u64, NodeId)> {
        std::mem::take(&mut self.lock().made)
    }

    #[must_use]
    pub fn take_settled(&self) -> Vec<Settled> {
        std::mem::take(&mut self.lock().settled)
    }
}

impl EditSource for EditQueue {
    fn absorb(&mut self, eng: &mut crate::Engine) -> Option<u64> {
        let edits = std::mem::take(&mut self.lock().pending);
        let mut last = None;
        for (e, tag) in edits {
            let made = match e {
                Edit::Push { unit, op, args } => Some(eng.push(unit, op, args)),
                Edit::InsertAfter { after, op, args } => Some(eng.insert_after(after, op, args)),
                Edit::InsertBefore { before, op, args } => Some(eng.insert_before(before, op, args)),
                Edit::Replace { node, op, args } => {
                    eng.replace(node, op, args);
                    None
                }
                Edit::Remove { node } => {
                    eng.remove(node);
                    None
                }
                Edit::Environment { fields } => {
                    eng.set_environment(fields);
                    None
                }
                Edit::Mark { node, on } => {
                    if on {
                        eng.mark(node);
                    } else {
                        eng.unmark(node);
                    }
                    None
                }
            };
            let mut st = self.lock();
            st.applied += 1;
            if let Some(n) = made {
                st.made.push((tag, n));
            }
            last = Some(tag);
        }
        last
    }

    fn settled(&mut self, _eng: &crate::Engine, items: &[Settled]) -> bool {
        self.lock().settled.extend_from_slice(items);
        true
    }
}
