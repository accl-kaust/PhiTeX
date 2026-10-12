//! The engine: program graph, def-use bindings over a bracket-structured
//! order, claims, scheduling, speculation and incremental update.
//!
//! # Model
//!
//! * A unit is an ordered sequence of nodes (an augmented treap keyed by
//!   order labels). Children unfold inline, right after their parent (and
//!   after any nodes it claims).
//! * Scopes are brackets: a node may open or close a scope (its *effect*). A
//!   local definition at `d` reaches a use at `u` iff `d` is at depth 0 or no
//!   unmatched close lies in `[d, u)`; the treap answers that in O(log n).
//!   Unmatched closes at depth 0 are ignored.
//! * A node may *claim* following siblings (a count, or up to a delimiter,
//!   found by the client's `claim` hook over the text). Claimed nodes are
//!   inert: they do not bind, run or define; the claimer receives their text.
//! * Structure that depends on a value (a dynamic definition set, effect or
//!   claim) is *pending* while its node is in work. A use waits only if its
//!   binding walk crosses a pending node, and only until that node resolves.
//!
//! # Invariants
//!
//! * Every node is `Done` or in work (`Dirty`, `Spec`, `Parked`); claimed
//!   nodes are `Inert`. Every change to a value, binding or structure marks
//!   exactly the affected readers dirty, so at quiescence every `Done` node's
//!   reads equal the current values of its current bindings.
//! * Heights order work: a `Done` reader is higher than what it read. A
//!   `Done` node lower than every queued node is settled; non-speculative
//!   runs read only settled values.
//! * Speculative runs read predictions and record each assumption on the
//!   in-work node it depends on. When that node finishes, all assumptions on
//!   it are checked at once; a run whose assumptions all hold is confirmed
//!   without re-running or re-walking.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use crate::ctx::{Ctx, Stop};
use crate::hash::{FxHashMap, FxHashSet, hash_bytes, hash_words};
use crate::obs::{count, span};
use crate::sched::{Candidate, Executor, Job, Scheduler};
use crate::tree::{Arena, Aug, LazyFold, NIL as TNIL};
use crate::value::{Field, Value, ValueId, Values};

/// A node reference: its slot index and the slot's generation when the node
/// was created. Freeing a node bumps its slot's generation, so a reference
/// kept anywhere never aliases a later node in the same slot.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub(crate) struct Slot {
    pub(crate) ix: u32,
    pub(crate) gen_: u32,
}

pub(crate) const NIL: Slot = Slot {
    ix: u32::MAX,
    gen_: u32::MAX,
};

impl Slot {
    /// Packed into a `u64` (for keys that hold either a field or a node).
    pub(crate) fn pack(self) -> u64 {
        (u64::from(self.gen_) << 32) | u64::from(self.ix)
    }

    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn unpack(k: u64) -> Slot {
        Slot {
            ix: k as u32,
            gen_: (k >> 32) as u32,
        }
    }
}

/// No `Aux` record.
const NO_AUX: u32 = u32::MAX;

/// Content-derived node identity: the same under every schedule.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct NodeId(pub(crate) u64);

impl NodeId {
    #[must_use]
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// An ordered unit of program.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct UnitId(pub(crate) u32);

/// A registered operation.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct OpId(pub(crate) u32);

/// Visibility of a definition.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum Locality {
    /// Reaches uses up to the close of the defining node's scope.
    Local,
    /// Reaches every later use.
    Global,
}

/// Scope effect of a node, from its text.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    None,
    /// Opens a scope after the node (`Brackets { opens: 1, .. }`).
    Open,
    /// Closes a scope after the node.
    Close,
    /// Known only when the node runs ([`Ctx::set_effect`]).
    Pending,
    /// The general form.
    Scope(Brackets),
}

/// A node's brackets: it closes `closes` scopes, then opens `opens`, either
/// after itself (like `begin`/`end`) or `before` itself (the node then lies
/// inside what it opens and outside what it closes). An opened scope may
/// carry a `kind` (an opaque client tag), which nodes inside it read with
/// [`Ctx::scope_kind`].
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Brackets {
    /// At most 3.
    pub closes: u8,
    /// At most 3.
    pub opens: u8,
    pub before: bool,
    pub kind: Option<ValueId>,
}

/// Extent of a node's claim on its following siblings, from the text.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Claim {
    None,
    /// The next `n` siblings (fewer if the unit ends).
    Count(usize),
    /// Known only when the node runs ([`Ctx::claim`]).
    Pending,
}

/// A stream claim's test of one candidate node (see [`Ctx::claim_stream`]).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Until {
    /// The claim goes on past it.
    No,
    /// The claim ends before it.
    Yes,
    /// Decided by [`Operation::until_value`] from the value `field` has at
    /// the candidate's position.
    Read(Field),
    /// Decided by [`Operation::until_values`] from the values each field
    /// has there: a test resting on several facts. Each is a recorded read,
    /// so a change in any one runs the claimer again.
    ReadEach(Vec<Field>),
}

/// What an operation's hooks say about a text from the text alone (see
/// [`Engine::statics`]).
pub(crate) struct Static {
    decl: Option<Vec<(Field, Locality)>>,
    eff: Effect,
    claim: Claim,
    reads: Option<Vec<Field>>,
    may_define: MayDefine,
    predict: bool,
}

/// Where a pending node is indexed: `pending` (an unbounded definition
/// set), `pending_eff` (a scope effect), and its bounded definition set.
#[derive(Clone, Debug)]
struct PendIx {
    unbounded: bool,
    effect: bool,
    bound: MayDefine,
}

/// What a node with a dynamic definition set may define, from its text.
///
/// Advisory: uses of fields outside the bound do not wait on the node; if it
/// defines one anyway, the uses it reaches are re-bound and re-run when it
/// finishes (speed, never results).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MayDefine {
    /// Nothing visible after the node.
    Nothing,
    /// Only these fields.
    Names(Vec<Field>),
    /// Only fields whose name starts with one of these.
    Prefixes(Vec<String>),
    /// Anything (filtered by [`Operation::may_define_field`]).
    Any,
}

/// A client operation.
///
/// The text-only hooks (`defs`, `may_define`, `effect`, `claim` with no
/// following text, `reads`, `until_predict` with no following text) are
/// called once per distinct text and their answers memoized: if what they
/// depend on besides the text changes, the client says so with
/// [`Engine::refresh_where`].
pub trait Operation: Send + Sync {
    fn name(&self) -> &'static str {
        "op"
    }

    /// The fields this node defines directly, if known from its text. `None`
    /// makes the set pending until the node runs.
    fn defs(&self, args: &[ValueId], values: &Values) -> Option<Vec<(Field, Locality)>>;

    /// For a node whose definition set is dynamic: what it could possibly
    /// define, if the text bounds it. The set is then pending only for those
    /// fields; other uses never wait on it.
    fn may_define(&self, _args: &[ValueId], _values: &Values) -> MayDefine {
        MayDefine::Any
    }

    /// For a node whose definition set is dynamic and unbounded: whether it
    /// could ever define `f` (a namespace rule of the language). Uses of
    /// fields it can never define do not wait on it.
    fn may_define_field(&self, _args: &[ValueId], _values: &Values, _f: Field) -> bool {
        true
    }

    /// The node's scope effect, from its text.
    fn effect(&self, _args: &[ValueId], _values: &Values) -> Effect {
        Effect::None
    }

    /// For a pending claim: the most following siblings it could ever take
    /// (a syntactic bound, e.g. "never past the block's close"); `None` if
    /// unbounded, including when the bound is not in the text yet. A pending claim makes waiters only of uses whose reaching
    /// definition it could swallow.
    fn claim_limit(
        &self,
        _args: &[ValueId],
        _values: &Values,
        _following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Option<usize> {
        None
    }

    /// The node's claim on following siblings: a syntactic scan of their text.
    fn claim(
        &self,
        _args: &[ValueId],
        _values: &Values,
        _following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        Claim::None
    }

    /// For a stream claimer ([`Ctx::claim_stream`]): the test of candidate
    /// node `cand` (its class is its operation and text).
    fn until(&self, _args: &[ValueId], _values: &Values, _cand: OpId, _cand_args: &[ValueId]) -> Until {
        Until::No
    }

    /// For [`Until::Read`]: whether the claim ends before `cand`, given the
    /// value the field has there.
    fn until_value(
        &self,
        _args: &[ValueId],
        _values: &Values,
        _cand: OpId,
        _cand_args: &[ValueId],
        _value: Option<ValueId>,
    ) -> bool {
        false
    }

    /// For [`Until::ReadEach`]: whether the claim ends before `cand`, given
    /// the values the fields have there, in order.
    fn until_values(
        &self,
        _args: &[ValueId],
        _values: &Values,
        _cand: OpId,
        _cand_args: &[ValueId],
        _fields: &[Option<ValueId>],
    ) -> bool {
        false
    }

    /// For a stream claimer: where its claim ends, predicted from the text of
    /// the following siblings (the index of the sibling it ends before).
    /// The engine then speculates on the claimer (under
    /// [`Speculation::Predicted`](crate::Speculation)), confirming the
    /// prediction by testing that sibling; a wrong prediction only costs a
    /// re-run.
    fn until_predict(
        &self,
        _args: &[ValueId],
        _values: &Values,
        _following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Option<usize> {
        None
    }

    /// Fields the node may read (a hint): a node whose hinted inputs are
    /// known to be missing is not started.
    fn reads(&self, _args: &[ValueId], _values: &Values) -> Option<Vec<Field>> {
        None
    }

    /// Runs the node.
    ///
    /// # Errors
    ///
    /// Propagates [`Stop`] from `cx` when a value is not available yet.
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop>;
}

/// An associative combine with an identity, for accumulator fields.
pub trait Monoid: Send + Sync {
    fn empty(&self, values: &Values) -> ValueId;
    fn combine(&self, values: &Values, a: ValueId, b: ValueId) -> ValueId;
}

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

/// Counters for one evaluation (including the edits before it).
///
/// The first group is canonical: it depends only on the program before and
/// after, never on the schedule, thread count or speculation policy. The
/// second group describes how the work was scheduled.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Nodes whose inputs changed (or that are new) and so had to run.
    pub nodes_run: u64,
    /// Of those, nodes whose outputs and definitions came out unchanged.
    pub cutoffs: u64,
    /// Uses whose reaching definition changed.
    pub rebinds: u64,
    /// Nodes created.
    pub created: u64,
    /// Nodes freed.
    pub freed: u64,
    /// Loop-back rounds.
    pub rounds: u64,
    /// Loop-back restarts from initial values.
    pub restarts: u64,
    /// `nodes_run` by operation name.
    pub runs_by_op: Vec<(&'static str, u64)>,

    /// Executions (complete runs).
    pub runs: u64,
    /// Executions stopped at a value that was not ready.
    pub stops: u64,
    /// Waits on pending structure (definition sets, claims, scope effects).
    pub pending_waits: u64,
    /// Nodes left waiting for an environment field not yet supplied (nor
    /// declared absent), and the nodes waiting on them, when the
    /// evaluation ended.
    pub env_stalled: u64,
    /// The nodes waiting for an environment field, with the field each
    /// one waits for (what the client has to supply or declare absent).
    pub env_waits: Vec<(NodeId, Field)>,
    /// Nodes held back before running because a hinted input was missing.
    pub gated: u64,
    /// Speculative runs kept (with outstanding assumptions).
    pub speculations: u64,
    /// Speculative runs confirmed.
    pub confirmations: u64,
    /// Speculative runs whose assumptions failed.
    pub mispredictions: u64,
    /// Speculative runs that ran out of fuel.
    pub fuel_outs: u64,
    /// Scheduler steps.
    pub steps: u64,
    /// Values freed by the sweep.
    pub values_freed: u64,
    /// Nodes re-bound because a scope boundary moved (region sizes).
    pub scope_region: u64,
    /// Pending definition sets resolved this evaluation (canonical).
    pub pending_sets: u64,
    /// Of those, the ones that defined anything visible after the node:
    /// their own definitions, or definitions of their unfolded text that
    /// reach past it (canonical).
    pub pending_sets_defining: u64,
    /// Unfolded-text memo lookups that found the text.
    pub text_memo_hits: u64,
    /// Unfolded-text memo lookups that produced it.
    pub text_memo_misses: u64,
    /// Lexical applications found in the memo.
    pub app_memo_hits: u64,
    /// Lexical applications created.
    pub app_memo_misses: u64,
    /// Speculative runs kept whose shape (children, claim, scope effect or
    /// definition set) differed from the node's previous shape.
    pub spec_shape_changes: u64,
    /// Nodes parked on a node of each operation (by name).
    pub waits_by_op: Vec<(&'static str, u64)>,
}

impl Report {
    /// The count for operation `name` in `runs_by_op` or `waits_by_op`.
    #[must_use]
    pub fn by_op(counts: &[(&'static str, u64)], name: &str) -> u64 {
        counts.iter().find(|c| c.0 == name).map_or(0, |c| c.1)
    }
}

fn bump(counts: &mut Vec<(&'static str, u64)>, name: &'static str) {
    match counts.iter_mut().find(|c| c.0 == name) {
        Some(c) => c.1 += 1,
        None => counts.push((name, 1)),
    }
}

/// Evaluation errors. An evaluation never runs on without progress:
/// each way it can fail to finish ends with the nodes and the reason
/// (see [`Limits`]).
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
    },
    /// [`Limits::total_runs`] or [`Limits::time`] was reached with these
    /// nodes still waiting or queued.
    Deadline {
        runs: u64,
        waiters: Vec<Waiter>,
    },
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
    /// The node it waited on finished (or went away).
    Woken { by: NodeId },
    /// A loop-back field's value for the next round.
    Loop { field: Field },
    /// The environment's `field` changed.
    Environment { field: Field },
    /// The structure around it changed (a claim, a scope, text coming or
    /// going, a range it reads).
    Structure,
}

/// What a node is waiting for when an evaluation stops without finishing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiter {
    pub node: NodeId,
    pub on: WaitOn,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitOn {
    /// Parked until that node finishes (its definitions, claim, scope
    /// effect or output).
    Node(NodeId),
    /// Parked until every node in its accumulator range has finished.
    Range { field: Field },
    /// Parked until the client supplies that environment field.
    Environment(Field),
    /// Not parked: dirty and queued, waiting for its turn.
    Queued,
}

/// Bounds on one evaluation; exceeding one ends it with an error naming
/// the nodes involved ([`Engine::set_limits`]).
#[derive(Clone, Debug, PartialEq, Eq)]
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

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvalError::Cycle(ns) => write!(f, "cycle among {} nodes", ns.len()),
            EvalError::NonConvergence { rounds } => write!(f, "no fixpoint after {rounds} rounds"),
            EvalError::ConflictingPublication { field } => write!(f, "conflicting publications for {field:?}"),
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
            EvalError::NoProgress { rounds, waiters } => {
                write!(
                    f,
                    "{rounds} rounds without progress; {} nodes waiting: {waiters:?}",
                    waiters.len()
                )
            }
            EvalError::Deadline { runs, waiters } => {
                write!(
                    f,
                    "deadline after {runs} runs; {} nodes waiting: {waiters:?}",
                    waiters.len()
                )
            }
        }
    }
}

impl std::error::Error for EvalError {}

// ------------------------------------------------------------------ internals

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum State {
    Dirty,
    Spec,
    Parked,
    Done,
    Inert,
    Free,
}

pub(crate) const K_FIELD: u8 = 0;
pub(crate) const K_OUTPUT: u8 = 1;
pub(crate) const K_LOOP: u8 = 2;
/// A link of a contribution chain past the first (see
/// [`Ctx::contributions_before_me`]): `key` is the sequence's hidden field.
pub(crate) const K_CHAIN: u8 = 3;
/// A read of the environment ([`Ctx::use_env`]): `key` is the field.
pub(crate) const K_ENV: u8 = 4;

/// A recorded read: 16 bytes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Use {
    /// Field (`Field`, `Loop`, `Acc`) or target slot (`Output`).
    pub(crate) key: u64,
    /// Def node (field reads); `NIL`: the unit's environment.
    pub(crate) site: Slot,
    pub(crate) kind: u8,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Def {
    pub(crate) field: Field,
    pub(crate) value: ValueId,
    pub(crate) global: bool,
    /// The scope depth a local definition is indexed at (0 for a global).
    pub(crate) depth: u32,
}

const F_RAN: u16 = 1;
const F_APP: u16 = 2;
const F_INPEND: u16 = 4;
const F_QUEUED: u16 = 8;
const F_PROV: u16 = 16;
const F_NOSPEC: u16 = 32;
const F_DYN: u16 = 64;
const F_EFF_DYN: u16 = 128;
const F_CLAIM_DYN: u16 = 256;
const F_TAINT: u16 = 512;
const F_CLAIMER: u16 = 1024;
const F_PARKSET: u16 = 2048;
/// Runs of one stream claimer in one evaluation taken as a cycle.
const STREAM_RUN_LIMIT: u32 = 4096;
/// A stream claimer with a predicted extent: eligible for speculation.
const F_PREDICT: u16 = 4096;
const F_HINT: u16 = 8192;
const F_MAYBE: u16 = 16384;
const PREFIX_SEED: u64 = 0x9e37_79b9;
/// Dirty only because its bindings moved: revalidated before it runs.
const F_REBIND: u16 = 32768;

const EFF_NONE: u8 = 0;
// Effect codes: closes in bits 0-1, opens in bits 2-3, `before` in bit 4.
const EFF_CLOSE: u8 = 1;
const EFF_OPEN: u8 = 4;
const EFF_BEFORE: u8 = 16;

fn eff_closes(e: u8) -> u32 {
    u32::from(e & 3)
}

fn eff_opens(e: u8) -> u32 {
    u32::from((e >> 2) & 3)
}

/// The node record (under 128 bytes; rare data lives in `Aux`).
pub(crate) struct Node {
    pub(crate) id: NodeId,
    pub(crate) args: ValueId,
    pub(crate) output: ValueId,
    pub(crate) label: u64,
    pub(crate) sig: u64,
    pub(crate) op: OpId,
    pub(crate) unit: u32,
    pub(crate) parent: Slot,
    pub(crate) height: u32,
    pub(crate) floor: u32,
    pub(crate) epoch: u32,
    /// The slot's generation (see [`Slot`]).
    gen_: u32,
    aux: u32,
    on: Slot,
    pub(crate) state: State,
    pub(crate) effect: u8,
    /// The effect the bindings reflect (what the order holds while a
    /// pending effect is decided).
    bound: u8,
    flags: u16,
    pub(crate) uses: Box<[Use]>,
    pub(crate) defs: Box<[Def]>,
}

impl Node {
    pub(crate) fn has(&self, f: u16) -> bool {
        self.flags & f != 0
    }

    pub(crate) fn def_value(&self, f: Field) -> Option<ValueId> {
        self.defs
            .iter()
            .find(|d| d.field == f)
            .map(|d| d.value)
            .filter(|v| *v != ValueId::NONE)
    }

    pub(crate) fn has_dyn_effect(&self) -> bool {
        self.has(F_EFF_DYN)
    }

    pub(crate) fn in_work(&self) -> bool {
        matches!(self.state, State::Dirty | State::Spec | State::Parked)
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum AssumeKind {
    Def(Field),
    Output,
    Structure(Field),
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct Assume {
    node: Slot,
    epoch: u32,
    kind: AssumeKind,
    value: ValueId,
}

#[derive(Default)]
pub(crate) struct Aux {
    pub(crate) children: Vec<Slot>,
    waiters: Vec<Slot>,
    out_readers: Vec<Slot>,
    publishes: Vec<(Field, ValueId)>,
    contribs: Vec<(Field, ValueId)>,
    pub(crate) claimed: Vec<Slot>,
    /// What a pending node is indexed under.
    pend_fields: Option<PendIx>,
    claimed_by: Option<Slot>,
    /// The kind of the scope this node opens (`None`: no kind).
    kind: Option<ValueId>,
    pub(crate) body: Option<u32>,
    assumers: Vec<Assume>,
    assume_left: u32,
    watch: Option<(Field, Slot)>,
    /// For a pending claim: 1 + the label of the last sibling it could take
    /// (0: not computed yet, i.e. unbounded).
    reach1: u64,
    /// For a stream claimer: the range whose structure its last run read.
    stream: Option<StreamRange>,
}

/// The range a stream claim read, as nodes (labels change under
/// renumbering): after `after`'s label, up to and including the node
/// after `end` (the delimiter; `NIL`: to the end of the unit).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct StreamRange {
    pub(crate) after: Slot,
    pub(crate) end: Slot,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct OItem {
    slot: Slot,
    eff: u8,
    pend: bool,
    work: bool,
    h: u32,
    reach: u64,
    /// `reach`, or 0 while the claimer waits for a recompute: a structure
    /// edit hits each pending claim once per flush.
    hit: u64,
    /// The last label a stream claimer's recorded range covers (0: none).
    sreach: u64,
}

impl From<OItem> for Slot {
    fn from(i: OItem) -> Slot {
        i.slot
    }
}

/// Order summary: bracket counts, in-work count, max height, pending count.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct OSum {
    c: u32,
    o: u32,
    work: u32,
    maxh: u32,
    pend: u32,
    maxr: u64,
    maxhit: u64,
    maxs: u64,
}

struct OAug;

impl Aug<OItem, OSum> for OAug {
    fn empty(&self) -> OSum {
        OSum::default()
    }

    fn leaf(&self, i: &OItem) -> OSum {
        OSum {
            c: eff_closes(i.eff),
            o: eff_opens(i.eff),
            work: u32::from(i.work),
            maxh: i.h,
            pend: u32::from(i.pend),
            maxr: i.reach,
            maxhit: i.hit,
            maxs: i.sreach,
        }
    }

    fn combine(&self, a: OSum, b: OSum) -> OSum {
        OSum {
            c: a.c + b.c.saturating_sub(a.o),
            o: b.o + a.o.saturating_sub(b.c),
            work: a.work + b.work,
            maxh: a.maxh.max(b.maxh),
            pend: a.pend + b.pend,
            maxr: a.maxr.max(b.maxr),
            maxhit: a.maxhit.max(b.maxhit),
            maxs: a.maxs.max(b.maxs),
        }
    }
}

/// Accumulator subtree summary: a polynomial content hash of the
/// contribution sequence (two bases, mod 2^61 - 1), associative and so
/// independent of the tree's shape. Cheap to combine; the actual monoid value
/// is built only when read, and memoized by this summary.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub(crate) struct AccSum {
    h1: u64,
    h2: u64,
    p1: u64,
    p2: u64,
    len: u64,
}

const M61: u64 = (1 << 61) - 1;

fn mulmod(a: u64, b: u64) -> u64 {
    let x = u128::from(a) * u128::from(b);
    #[allow(clippy::cast_possible_truncation)]
    let lo = (x as u64) & M61;
    #[allow(clippy::cast_possible_truncation)]
    let hi = (x >> 61) as u64;
    let r = lo + hi;
    if r >= M61 { r - M61 } else { r }
}

fn addmod(a: u64, b: u64) -> u64 {
    let r = a + b;
    if r >= M61 { r - M61 } else { r }
}

const B1: u64 = 0x1F35_A6E1_99C4_7D2B % M61;
const B2: u64 = 0x0B7E_1516_28AE_D2A7 % M61;

struct AccAug;

impl Aug<ValueId, AccSum> for AccAug {
    fn empty(&self) -> AccSum {
        AccSum {
            p1: 1,
            p2: 1,
            ..AccSum::default()
        }
    }

    fn leaf(&self, v: &ValueId) -> AccSum {
        AccSum {
            h1: hash_words(41, &[v.raw()]) % M61,
            h2: hash_words(42, &[v.raw()]) % M61,
            p1: B1,
            p2: B2,
            len: 1,
        }
    }

    fn combine(&self, a: AccSum, b: AccSum) -> AccSum {
        AccSum {
            h1: addmod(mulmod(a.h1, b.p1), b.h1),
            h2: addmod(mulmod(a.h2, b.p2), b.h2),
            p1: mulmod(a.p1, b.p1),
            p2: mulmod(a.p2, b.p2),
            len: a.len + b.len,
        }
    }
}

/// Builds monoid values from the tree, reusing every whole subtree whose
/// content was built before.
struct AccBuild<'a> {
    m: &'a dyn Monoid,
    v: &'a Values,
    empty: ValueId,
    cache: &'a RwLock<FxHashMap<AccSum, ValueId>>,
}

impl LazyFold<ValueId, AccSum, ValueId> for AccBuild<'_> {
    fn empty(&self) -> ValueId {
        self.empty
    }

    fn leaf(&self, i: &ValueId) -> ValueId {
        *i
    }

    fn combine(&self, a: ValueId, b: ValueId) -> ValueId {
        if a == self.empty {
            b
        } else if b == self.empty {
            a
        } else {
            self.m.combine(self.v, a, b)
        }
    }

    fn whole(&self, sum: &AccSum, build: &mut dyn FnMut() -> ValueId) -> ValueId {
        if sum.len <= 1 {
            return build();
        }
        if let Some(&v) = self.cache.read().unwrap_or_else(PoisonError::into_inner).get(sum) {
            return v;
        }
        let v = build();
        self.cache
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*sum, v);
        v
    }
}

#[derive(Copy, Clone, Debug)]
struct Watch {
    field: Field,
    anchor: Slot,
    seen: ValueId,
    waiting: bool,
}

pub(crate) struct Unit {
    pub(crate) root: u32,
    pub(crate) env: FxHashMap<Field, ValueId>,
    owner: Slot,
    accs: FxHashMap<Field, u32>,
    watches: BTreeMap<(u64, Slot), Watch>,
    max_span: u64,
    live: bool,
}

#[derive(Clone)]
struct Snap {
    existed: bool,
    sig: u64,
    args: ValueId,
    out: u64,
    sites: Vec<(u64, u64)>,
}

#[derive(Copy, Clone)]
pub(crate) struct WalkResult {
    pub(crate) site: Slot,
    pub(crate) pending: Slot,
}

const GAP: u64 = 1 << 32;
const START_SITE: u64 = 1 << 40;
const DEFAULT_MAX_ROUNDS: u64 = 64;

fn site_key(unit: u32, site: Slot) -> u64 {
    if site == NIL {
        START_SITE | u64::from(unit)
    } else {
        site.pack()
    }
}

/// The evaluator.
pub struct Engine {
    pub(crate) values: Values,
    ops: Vec<Arc<dyn Operation>>,
    pub(crate) nodes: Vec<Node>,
    /// Indices of free slots.
    free_slots: Vec<u32>,
    pub(crate) aux: Vec<Aux>,
    free_aux: Vec<u32>,
    ids: FxHashMap<NodeId, Slot>,
    pub(crate) units: Vec<Unit>,
    free_units: Vec<u32>,
    pub(crate) order: Arena<OItem, OSum>,
    accs_arena: Arena<ValueId, AccSum>,
    /// Built accumulator values of whole subtrees, by content summary
    /// (cleared at value sweeps).
    acc_cache: RwLock<FxHashMap<AccSum, ValueId>>,
    monoids: FxHashMap<Field, Arc<dyn Monoid>>,
    defs_ix: BTreeMap<(u32, Field, u64), Slot>,
    /// Local definitions by scope depth: `(unit, field, depth, label)`.
    local_ix: BTreeMap<(u32, Field, u32, u64), Slot>,
    /// The depth each local definition is indexed under.
    /// Runs of each stream claimer in this evaluation.
    stream_runs: FxHashMap<Slot, u32>,
    /// The labels `stream_touch` saw this evaluation, in order: a claim
    /// that ran meanwhile is checked against those after its start.
    pub(crate) touch_log: Vec<(u32, u64)>,
    in_eval: bool,
    /// Label renumberings so far: a stream claim's run that spans one has
    /// read labels that no longer mean the same.
    pub(crate) relabels: u64,
    /// Set while a balanced, newly created text gets its scope kinds: a
    /// local definition there has no readers yet.
    fresh_scope: bool,
    /// The environment: fields defined before every text unit (text order
    /// -1), below the unit's own [`Engine::set_env`] entries.
    environment: FxHashMap<Field, ValueId>,
    /// The environment for the next evaluation, applied at its start.
    env_next: Option<FxHashMap<Field, ValueId>>,
    /// Environment fields declared absent ([`Ctx::use_env`] reads them as
    /// `None` instead of waiting), and those for the next evaluation.
    env_absent: FxHashSet<Field>,
    env_absent_next: Option<FxHashSet<Field>>,
    /// Nodes parked on an environment field neither supplied nor absent.
    env_waiters: FxHashMap<Field, Vec<Slot>>,
    limits: Limits,
    /// Per evaluation: applied runs per node, the causes of the last runs
    /// of nodes past half their budget (`tracing` says any exist),
    /// children created per node, rounds in a row applying nothing, runs
    /// in all, and the start time.
    runs_in_eval: FxHashMap<Slot, u32>,
    causes: FxHashMap<Slot, Vec<Cause>>,
    /// Nodes past half their run budget (`> 0`: causes are recorded).
    tracing: u32,
    created_in_eval: FxHashMap<Slot, (u32, Vec<(OpId, ValueId)>)>,
    /// While a run of fresh siblings is bound: (label, effect in the order,
    /// scope depth before it), sorted by label. Their definitions' depths
    /// come from here instead of a fold each.
    run_depths: Vec<(u64, u8, u32)>,
    idle_rounds: u32,
    eval_runs: u64,
    #[cfg(not(target_arch = "wasm32"))]
    eval_start: Option<std::time::Instant>,
    /// Global definitions: `(unit, field, label)`.
    global_ix: BTreeMap<(u32, Field, u64), Slot>,
    users: BTreeMap<(u64, Field, u64), Slot>,
    /// Pending nodes with an unbounded definition set.
    pending: BTreeMap<(u32, u64), Slot>,
    /// Pending nodes whose scope effect is unknown.
    pending_eff: BTreeMap<(u32, u64), Slot>,
    /// Pending definition sets bounded to known fields.
    pending_f: BTreeMap<(u32, Field, u64), Slot>,
    /// Pending definition sets bounded to name prefixes: `(unit, prefix
    /// hash, label)`, with how many entries use each prefix length.
    pending_p: BTreeMap<(u32, u64, u64), Slot>,
    pending_p_lens: BTreeMap<usize, u32>,
    pub(crate) loop_vals: FxHashMap<Field, ValueId>,
    loop_init: FxHashMap<Field, ValueId>,
    loop_users: FxHashMap<Field, FxHashSet<Slot>>,
    loop_pubs: FxHashMap<Field, BTreeMap<NodeId, ValueId>>,
    pub(crate) memo: FxHashMap<(OpId, ValueId), Slot>,
    pub(crate) text_memo: RwLock<FxHashMap<(ValueId, ValueId), ValueId>>,
    /// The text-only hooks of each (operation, text), computed once.
    statics: RwLock<FxHashMap<(OpId, ValueId), Arc<Static>>>,
    /// Dirty demanded nodes by height (a bucket queue: entries are removed
    /// lazily, an entry is live iff its node is `F_QUEUED` at that height).
    queue: Vec<Vec<(NodeId, Slot)>>,
    /// No live queue entry below this height.
    queue_min: usize,
    /// Live queue entries.
    queue_len: usize,
    /// Slots whose order summary (work, height) is stale; repaired in one
    /// pass before anything reads those summaries.
    order_dirty: Vec<Slot>,
    /// Finished positions whose range readers are checked at the end of the
    /// batch.
    settle_pending: Vec<(u32, u64)>,
    parked: BTreeSet<(NodeId, Slot)>,
    roots: FxHashSet<Slot>,
    /// The hidden field an opener defines inside its scope: its kind.
    pub(crate) kind_field: Field,
    /// Contribution sequences: `(sequence, hidden field)`. A contributor
    /// defines the hidden field (its element, see `Contribution`); an opener
    /// defines it inside its scope as `seq_start`.
    seqs: Vec<(Field, Field)>,
    pub(crate) seq_start: ValueId,
    /// Live memoized applications, and how many there were after the last
    /// collection (see `collect_apps`).
    apps_live: usize,
    apps_at_collect: usize,
    claim_dirty: FxHashSet<Slot>,
    errors: Vec<EvalError>,
    pub(crate) level: u32,
    report: Report,
    touched: FxHashMap<NodeId, Snap>,
    max_rounds: u64,
    client_serial: u64,
    feedback: bool,
    spec_live: u32,
    /// Nodes committed from a speculative run this evaluation (a later
    /// re-dirtying is a misprediction).
    spec_done: FxHashSet<Slot>,
    pins: FxHashMap<ValueId, u32>,
    values_at_sweep: usize,
    open_op: OpId,
    close_op: OpId,
}

/// The built-in scope markers.
struct Marker(Effect);

impl Operation for Marker {
    fn name(&self) -> &'static str {
        "marker"
    }

    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        self.0
    }

    fn run(&self, _: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// Iterates the nodes following a node as text, skipping what lies inside
/// them: its later siblings, then (an unfolded child's text sits inline
/// after its parent) the parent's later siblings, and so on up.
pub(crate) struct Following<'a> {
    eng: &'a Engine,
    unit: u32,
    /// The label the next sibling is searched after.
    pub(crate) label: u64,
    parent: Slot,
    pub(crate) slots: Vec<Slot>,
}

impl Iterator for Following<'_> {
    type Item = (OpId, Vec<ValueId>);

    fn next(&mut self) -> Option<Self::Item> {
        let e = self.eng;
        loop {
            let (k, item) = e.order.next(e.units[self.unit as usize].root, self.label)?;
            self.label = k;
            let s = item.slot;
            let n = &e.nodes[s.ix as usize];
            if n.parent == self.parent {
                self.slots.push(s);
                return Some((n.op, e.args_vec(n.args)));
            }
            while !e.descends(s, self.parent) {
                // Past the parent's text: on to the parent's siblings.
                self.parent = e.nodes[self.parent.ix as usize].parent;
                if n.parent == self.parent {
                    self.slots.push(s);
                    return Some((n.op, e.args_vec(n.args)));
                }
            }
        }
    }
}

impl Engine {
    #[must_use]
    pub fn new() -> Self {
        let values = Values::new();
        let kind_field = values.field("\u{1}scope-kind");
        let mut e = Engine {
            values,
            ops: Vec::new(),
            nodes: Vec::new(),
            free_slots: Vec::new(),
            aux: Vec::new(),
            free_aux: Vec::new(),
            ids: FxHashMap::default(),
            units: Vec::new(),
            free_units: Vec::new(),
            order: Arena::default(),
            accs_arena: Arena::default(),
            acc_cache: RwLock::new(FxHashMap::default()),
            monoids: FxHashMap::default(),
            defs_ix: BTreeMap::new(),
            local_ix: BTreeMap::new(),
            stream_runs: FxHashMap::default(),
            touch_log: Vec::new(),
            in_eval: false,
            relabels: 0,
            fresh_scope: false,
            environment: FxHashMap::default(),
            env_next: None,
            env_absent: FxHashSet::default(),
            env_absent_next: None,
            env_waiters: FxHashMap::default(),
            limits: Limits::default(),
            runs_in_eval: FxHashMap::default(),
            causes: FxHashMap::default(),
            tracing: 0,
            created_in_eval: FxHashMap::default(),
            run_depths: Vec::new(),
            idle_rounds: 0,
            eval_runs: 0,
            #[cfg(not(target_arch = "wasm32"))]
            eval_start: None,
            global_ix: BTreeMap::new(),
            users: BTreeMap::new(),
            pending: BTreeMap::new(),
            pending_eff: BTreeMap::new(),
            pending_f: BTreeMap::new(),
            pending_p: BTreeMap::new(),
            pending_p_lens: BTreeMap::new(),
            loop_vals: FxHashMap::default(),
            loop_init: FxHashMap::default(),
            loop_users: FxHashMap::default(),
            loop_pubs: FxHashMap::default(),
            memo: FxHashMap::default(),
            text_memo: RwLock::new(FxHashMap::default()),
            statics: RwLock::new(FxHashMap::default()),
            queue: Vec::new(),
            queue_min: 0,
            queue_len: 0,
            order_dirty: Vec::new(),
            settle_pending: Vec::new(),
            parked: BTreeSet::new(),
            roots: FxHashSet::default(),
            kind_field: Field(ValueId::NONE),
            seqs: Vec::new(),
            seq_start: ValueId::NONE,
            apps_live: 0,
            apps_at_collect: 0,
            claim_dirty: FxHashSet::default(),
            errors: Vec::new(),
            level: 0,
            report: Report::default(),
            touched: FxHashMap::default(),
            max_rounds: DEFAULT_MAX_ROUNDS,
            client_serial: 0,
            feedback: false,
            spec_live: 0,
            spec_done: FxHashSet::default(),
            pins: FxHashMap::default(),
            values_at_sweep: 0,
            open_op: OpId(0),
            close_op: OpId(0),
        };
        e.open_op = e.register(Arc::new(Marker(Effect::Open)));
        e.close_op = e.register(Arc::new(Marker(Effect::Close)));
        e.kind_field = kind_field;
        e.seq_start = e.values.str("\u{1}start");
        e
    }

    /// Declares a contribution sequence: nodes add to it, read the element
    /// before them in their scope, take it away, or read the whole prefix
    /// (see [`Ctx::push_contribution`]).
    pub fn declare_sequence(&mut self, seq: Field) {
        if self.seq_field(seq).is_none() {
            let name = format!("\u{1}seq:{}", self.values.render(seq.0));
            let h = self.values.field(&name);
            self.seqs.push((seq, h));
        }
    }

    /// The hidden field of a declared sequence.
    pub(crate) fn seq_field(&self, seq: Field) -> Option<Field> {
        self.seqs.iter().find(|s| s.0 == seq).map(|s| s.1)
    }

    /// The field a client declaration names: a sequence's contributions
    /// are its hidden field.
    fn canon(&self, f: Field) -> Field {
        self.seq_field(f).unwrap_or(f)
    }

    /// The client name of a field (a sequence for its hidden field).
    fn client_name(&self, f: Field) -> Field {
        self.seqs.iter().find(|s| s.1 == f).map_or(f, |s| s.0)
    }

    fn is_seq_hidden(&self, f: Field) -> bool {
        self.seqs.iter().any(|s| s.1 == f)
    }

    /// The text-only hooks of `op` on `args`: computed on first use (by
    /// any thread) and shared by every node with that text.
    pub(crate) fn statics(&self, op: OpId, args: ValueId) -> Arc<Static> {
        if let Some(st) = self
            .statics
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(op, args))
        {
            return st.clone();
        }
        let av = self.args_vec(args);
        let opr = self.op(op);
        let v = &self.values;
        let decl = opr.defs(&av, v);
        let st = Arc::new(Static {
            may_define: if decl.is_none() {
                opr.may_define(&av, v)
            } else {
                MayDefine::Nothing
            },
            decl,
            eff: opr.effect(&av, v),
            claim: opr.claim(&av, v, &mut std::iter::empty()),
            reads: opr.reads(&av, v),
            predict: opr.until_predict(&av, v, &mut std::iter::empty()).is_some(),
        });
        self.statics
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry((op, args))
            .or_insert(st)
            .clone()
    }

    /// Drops the memoized hooks of texts no longer live; keeps the fields
    /// the rest name.
    fn statics_live(&mut self, live: &mut FxHashSet<ValueId>) {
        let st = self.statics.get_mut().unwrap_or_else(PoisonError::into_inner);
        st.retain(|(_, a), _| live.contains(a));
        for x in st.values() {
            let fields = x
                .decl
                .iter()
                .flatten()
                .map(|d| d.0)
                .chain(x.reads.iter().flatten().copied());
            live.extend(fields.map(|f| f.0));
            if let MayDefine::Names(fs) = &x.may_define {
                live.extend(fs.iter().map(|f| f.0));
            }
        }
    }

    /// Whether `s`'s operation statically declares (hidden) field `f`.
    fn declares(&self, s: Slot, f: Field) -> bool {
        let n = self.n(s);
        self.statics(n.op, n.args)
            .decl
            .as_ref()
            .is_some_and(|d| d.iter().any(|&(g, _)| self.canon(g) == f))
    }

    /// Whether `x`'s definition of `f` is the client's: not a scope kind,
    /// and not an opener's start-of-sequence marker (an opener's
    /// contributions are not supported: its markers take precedence). A
    /// marker left until the node's new effect applies has the value
    /// `seq_start`.
    fn is_client_def(&self, x: Slot, f: Field) -> bool {
        f != self.kind_field && !(self.is_seq_hidden(f) && eff_opens(self.oitem(x).eff) > 0)
    }

    /// Fields the engine defines on its own (never client-declared).
    fn is_hidden(&self, f: Field) -> bool {
        f == self.kind_field || self.is_seq_hidden(f)
    }

    /// The value table.
    #[must_use]
    pub fn values(&self) -> &Values {
        &self.values
    }

    /// Registers an operation.
    ///
    /// # Panics
    ///
    /// If more than `u32::MAX` operations are registered.
    pub fn register(&mut self, op: Arc<dyn Operation>) -> OpId {
        self.ops.push(op);
        OpId(u32::try_from(self.ops.len() - 1).expect("too many ops"))
    }

    /// The built-in operation that opens a scope.
    #[must_use]
    pub fn open_op(&self) -> OpId {
        self.open_op
    }

    /// The built-in operation that closes a scope.
    #[must_use]
    pub fn close_op(&self) -> OpId {
        self.close_op
    }

    /// Bounds an evaluation (see [`Limits`]); exceeding one is an error.
    pub fn set_limits(&mut self, limits: Limits) {
        self.limits = limits;
    }

    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Records why `x` is about to run again, for nodes past half their
    /// run budget (the last four causes are kept).
    fn note(&mut self, x: Slot, cause: Cause) {
        if self.tracing == 0 || !self.live(x) {
            return;
        }
        let half = self.limits.runs_per_node / 2;
        if self.runs_in_eval.get(&x).is_none_or(|&c| c < half) {
            return;
        }
        let v = self.causes.entry(x).or_default();
        if v.len() == 4 {
            v.remove(0);
        }
        v.push(cause);
    }

    /// The nodes an unfinished evaluation leaves waiting or queued.
    fn waiters(&self) -> Vec<Waiter> {
        let mut out: Vec<Waiter> = Vec::new();
        for &(_, s) in &self.parked {
            if !self.live(s) {
                continue;
            }
            let n = self.n(s);
            let on = if n.on != NIL && self.live(n.on) {
                WaitOn::Node(self.n(n.on).id)
            } else if let Some((f, _)) = self.env_waiters.iter().find(|(_, xs)| xs.contains(&s)) {
                WaitOn::Environment(*f)
            } else if let Some((field, _)) = self.aux_of(s).and_then(|a| a.watch) {
                WaitOn::Range { field }
            } else {
                WaitOn::Queued
            };
            out.push(Waiter { node: n.id, on });
        }
        for level in &self.queue {
            for &(_, s) in level {
                if self.live(s) && self.n(s).state == State::Dirty {
                    out.push(Waiter {
                        node: self.n(s).id,
                        on: WaitOn::Queued,
                    });
                }
            }
        }
        out.sort_by_key(|w| w.node);
        out.dedup_by_key(|w| w.node);
        out
    }

    pub fn set_max_rounds(&mut self, rounds: u64) {
        self.max_rounds = rounds.max(1);
    }

    /// Keeps `v` alive across value sweeps (for values the client holds).
    pub fn pin(&mut self, v: ValueId) {
        *self.pins.entry(v).or_default() += 1;
    }

    pub fn unpin(&mut self, v: ValueId) {
        if let Some(c) = self.pins.get_mut(&v) {
            *c -= 1;
            if *c == 0 {
                self.pins.remove(&v);
            }
        }
    }

    // ----------------------------------------------------------------- access

    pub(crate) fn n(&self, s: Slot) -> &Node {
        let n = &self.nodes[s.ix as usize];
        debug_assert_eq!(n.gen_, s.gen_, "stale node reference");
        n
    }

    fn nm(&mut self, s: Slot) -> &mut Node {
        let n = &mut self.nodes[s.ix as usize];
        debug_assert_eq!(n.gen_, s.gen_, "stale node reference");
        n
    }

    /// Whether `s` refers to a node that still exists (the same one).
    pub(crate) fn live(&self, s: Slot) -> bool {
        self.nodes
            .get(s.ix as usize)
            .is_some_and(|n| n.gen_ == s.gen_ && n.state != State::Free)
    }

    /// Internal consistency after an evaluation (for tests and fuzzing):
    /// every local definition indexed at its current depth, every settled
    /// use bound where a fresh walk binds it and indexed among its site's
    /// users, every decided pending effect reflected in the order.
    #[doc(hidden)]
    #[must_use]
    pub fn check_invariants(&self) -> Vec<String> {
        let mut bad = Vec::new();
        for i in 0..self.nodes.len() {
            let s = self.slot_at(i);
            if !self.live(s) {
                continue;
            }
            let n = self.n(s);
            if n.state == State::Inert || n.has(F_APP) {
                continue;
            }
            let root = self.units[n.unit as usize].root;
            for d in &n.defs {
                if d.global {
                    if self.global_ix.get(&(n.unit, d.field, n.label)) != Some(&s) {
                        bad.push(format!("{s:?}: global {:?} not indexed", d.field));
                    }
                    continue;
                }
                let want = self.def_depth(root, n.label, d.field);
                if d.depth != want {
                    bad.push(format!(
                        "{s:?}: local {} indexed at depth {}, now {want}",
                        self.values.render(d.field.0),
                        d.depth
                    ));
                }
            }
            if n.state != State::Done {
                continue;
            }
            if n.has(F_EFF_DYN) && n.bound != n.effect {
                bad.push(format!("{s:?}: bound effect {} but decided {}", n.bound, n.effect));
            }
            for u in &n.uses {
                if u.kind != K_FIELD {
                    continue;
                }
                let f = Field(ValueId(u.key));
                let w = self.walk(n.unit, n.label, f);
                if w.site != u.site {
                    let desc = |x: Slot| {
                        if self.live(x) {
                            let a = self.args_vec(self.n(x).args);
                            format!(
                                "{x:?}@{} {}",
                                self.n(x).label >> 32,
                                a.first().map_or(String::new(), |&a| self.values.render(a))
                            )
                        } else {
                            format!("{x:?}")
                        }
                    };
                    bad.push(format!(
                        "{}: use of {} bound to {}, walk finds {}",
                        desc(s),
                        self.values.render(f.0),
                        desc(u.site),
                        desc(w.site)
                    ));
                }
                if self.users.get(&(site_key(n.unit, u.site), f, n.label)) != Some(&s) {
                    bad.push(format!("{s:?}: use of {} missing from users", self.values.render(f.0)));
                }
            }
        }
        bad
    }

    /// A node's scheduling state, for diagnostics.
    #[doc(hidden)]
    #[must_use]
    pub fn debug_node(&self, id: NodeId) -> String {
        let Some(&s) = self.ids.get(&id) else {
            return "gone".into();
        };
        let n = self.n(s);
        let on = if n.on != NIL && self.live(n.on) {
            let o = self.n(n.on);
            format!(
                "{:?} state {:?} flags {:#x} args {}",
                o.id,
                o.state,
                o.flags,
                self.values.render(o.args)
            )
        } else {
            format!("{:?}", n.on)
        };
        format!("{s:?} state {:?} flags {:#x} on {on}", n.state, n.flags)
    }

    /// The current reference to slot `i`.
    #[allow(clippy::cast_possible_truncation)]
    fn slot_at(&self, i: usize) -> Slot {
        Slot {
            ix: i as u32,
            gen_: self.nodes[i].gen_,
        }
    }

    pub(crate) fn aux_of(&self, s: Slot) -> Option<&Aux> {
        let a = self.n(s).aux;
        if a == NO_AUX { None } else { Some(&self.aux[a as usize]) }
    }

    fn ax(&mut self, s: Slot) -> &mut Aux {
        if self.n(s).aux == NO_AUX {
            let i = if let Some(i) = self.free_aux.pop() {
                self.aux[i as usize] = Aux::default();
                i
            } else {
                self.aux.push(Aux::default());
                u32::try_from(self.aux.len() - 1).expect("aux overflow")
            };
            self.nm(s).aux = i;
        }
        let a = self.n(s).aux;
        &mut self.aux[a as usize]
    }

    pub(crate) fn children_of(&self, s: Slot) -> &[Slot] {
        self.aux_of(s).map_or(&[], |a| &a.children)
    }

    pub(crate) fn op(&self, op: OpId) -> &Arc<dyn Operation> {
        &self.ops[op.0 as usize]
    }

    pub(crate) fn args_vec(&self, args: ValueId) -> Vec<ValueId> {
        match &*self.values.get(args) {
            Value::List(xs) => xs.to_vec(),
            _ => Vec::new(),
        }
    }

    fn slot_of(&self, id: NodeId) -> Slot {
        *self.ids.get(&id).expect("unknown or freed node")
    }

    pub(crate) fn settled(&self, s: Slot) -> bool {
        let n = self.n(s);
        n.state == State::Done && n.height < self.level
    }

    fn descends(&self, mut s: Slot, anc: Slot) -> bool {
        if anc == NIL {
            return true;
        }
        while s != NIL {
            if s == anc {
                return true;
            }
            s = self.n(s).parent;
        }
        false
    }

    fn ord_fold(&self, unit: u32, lo: u64, hi: u64) -> OSum {
        self.order.fold(&OAug, self.units[unit as usize].root, lo, hi)
    }

    // ----------------------------------------------------------- public reads

    /// What a node is, for naming it in messages: its operation, its
    /// arguments, its unit, its parent (the node whose run made it) and
    /// its state. `None` for a node that is gone.
    #[must_use]
    pub fn node_info(&self, node: NodeId) -> Option<NodeInfo> {
        let &s = self.ids.get(&node)?;
        if !self.live(s) {
            return None;
        }
        let n = self.n(s);
        let state = match n.state {
            State::Dirty | State::Spec => NodeState::Dirty,
            State::Parked => NodeState::Parked,
            State::Done => NodeState::Done,
            State::Inert => NodeState::Inert,
            State::Free => return None,
        };
        Some(NodeInfo {
            op: self.op(n.op).name(),
            args: self.args_vec(n.args),
            unit: UnitId(n.unit),
            parent: (n.parent != NIL && self.live(n.parent)).then(|| self.n(n.parent).id),
            state,
        })
    }

    /// The output of `node` from its last completed run.
    #[must_use]
    pub fn output(&self, node: NodeId) -> Option<ValueId> {
        let s = *self.ids.get(&node)?;
        let n = self.n(s);
        (n.has(F_RAN) && n.state != State::Inert).then_some(n.output)
    }

    /// The value `node` defines for `field`.
    #[must_use]
    pub fn def_value(&self, node: NodeId, field: Field) -> Option<ValueId> {
        self.ids.get(&node).and_then(|&s| self.n(s).def_value(field))
    }

    /// The children `node` unfolded into.
    #[must_use]
    pub fn children(&self, node: NodeId) -> Vec<NodeId> {
        self.ids.get(&node).map_or_else(Default::default, |&s| {
            self.children_of(s).iter().map(|&c| self.n(c).id).collect()
        })
    }

    /// The nodes `node` claims.
    #[must_use]
    pub fn claimed(&self, node: NodeId) -> Vec<NodeId> {
        self.ids
            .get(&node)
            .and_then(|&s| self.aux_of(s))
            .map_or_else(Default::default, |a| a.claimed.iter().map(|&c| self.n(c).id).collect())
    }

    /// The node a use of `field` at `node` binds to (`None`: the unit's
    /// environment).
    #[must_use]
    pub fn reaching_def(&self, node: NodeId, field: Field) -> Option<NodeId> {
        let s = *self.ids.get(&node)?;
        let n = self.n(s);
        if n.has(F_APP) {
            return None;
        }
        let w = self.walk(n.unit, n.label, field);
        (w.site != NIL).then(|| self.n(w.site).id)
    }

    #[must_use]
    pub fn is_live(&self, node: NodeId) -> bool {
        self.ids.contains_key(&node)
    }

    #[must_use]
    pub fn live_nodes(&self) -> usize {
        self.ids.len()
    }

    /// The fold of an accumulator over a whole unit.
    #[must_use]
    pub fn acc_value(&self, unit: UnitId, field: Field) -> Option<ValueId> {
        self.monoids.get(&field)?;
        Some(self.acc_fold(unit.0, field, 0, u64::MAX))
    }

    /// The longest chain of dependent waits among live nodes (their maximum
    /// height after evaluation).
    #[must_use]
    pub fn critical_path(&self) -> u32 {
        self.nodes
            .iter()
            .filter(|n| n.state == State::Done)
            .map(|n| n.height)
            .max()
            .unwrap_or(0)
    }

    /// Bytes per node record and per use edge.
    #[must_use]
    pub fn record_sizes() -> (usize, usize) {
        (size_of::<Node>(), size_of::<Use>())
    }

    // ------------------------------------------------------------------ edits

    /// Creates an empty unit.
    ///
    /// # Panics
    ///
    /// If more than `u32::MAX` units are live.
    pub fn new_unit(&mut self) -> UnitId {
        UnitId(self.alloc_unit(NIL))
    }

    fn alloc_unit(&mut self, owner: Slot) -> u32 {
        let u = Unit {
            root: TNIL,
            env: FxHashMap::default(),
            owner,
            accs: FxHashMap::default(),
            watches: BTreeMap::new(),
            max_span: 0,
            live: true,
        };
        if let Some(i) = self.free_units.pop() {
            self.units[i as usize] = u;
            i
        } else {
            self.units.push(u);
            u32::try_from(self.units.len() - 1).expect("too many units")
        }
    }

    fn client_id(&mut self, unit: UnitId) -> NodeId {
        self.client_serial += 1;
        NodeId(hash_words(11, &[u64::from(unit.0), self.client_serial]))
    }

    /// Appends a node at the end of `unit`.
    pub fn push(&mut self, unit: UnitId, op: OpId, args: Vec<ValueId>) -> NodeId {
        span!(Edit);
        let after = self.order.last(self.units[unit.0 as usize].root).map(|(_, i)| i.slot);
        let after = after.map(|a| self.subtree_end(a));
        self.client_insert(unit, after, op, args)
    }

    /// Inserts a node right after `after` and everything it unfolded into or
    /// claims.
    pub fn insert_after(&mut self, after: NodeId, op: OpId, args: Vec<ValueId>) -> NodeId {
        span!(Edit);
        let a = self.slot_of(after);
        let unit = UnitId(self.n(a).unit);
        // Text order: a claimer's claimed text follows it directly (its
        // unfolded children come after that), so a line inserted after it
        // lands inside the claim.
        let end = if self.aux_of(a).is_some_and(|x| !x.claimed.is_empty()) {
            a
        } else {
            self.subtree_end(a)
        };
        self.client_insert(unit, Some(end), op, args)
    }

    /// Inserts a node right before `before`.
    pub fn insert_before(&mut self, before: NodeId, op: OpId, args: Vec<ValueId>) -> NodeId {
        span!(Edit);
        let b = self.slot_of(before);
        let n = self.n(b);
        let unit = UnitId(n.unit);
        let prev = self
            .order
            .prev(self.units[n.unit as usize].root, n.label)
            .map(|(_, i)| i.slot);
        self.client_insert(unit, prev, op, args)
    }

    fn client_insert(&mut self, unit: UnitId, after: Option<Slot>, op: OpId, args: Vec<ValueId>) -> NodeId {
        let id = self.client_id(unit);
        let args = self.values.list(args);
        let s = self.create_node(unit.0, after, op, args, NIL, id, false);
        self.structure_edit(s);
        id
    }

    /// Removes a client node with everything it unfolded into.
    ///
    /// # Panics
    ///
    /// If `node` was not created by the client.
    pub fn remove(&mut self, node: NodeId) {
        span!(Edit);
        let s = self.slot_of(node);
        assert!(
            self.n(s).parent == NIL && !self.n(s).has(F_APP),
            "only client nodes can be removed"
        );
        let unit = self.n(s).unit;
        let label = self.n(s).label;
        let prev = self
            .order
            .prev(self.units[unit as usize].root, label)
            .map(|(_, i)| i.slot);
        let by = self.aux_of(s).and_then(|a| a.claimed_by);
        self.dirty_reaching_claims(unit, label);
        self.roots.remove(&s);
        self.free_node(s);
        if let Some(c) = by.filter(|&c| self.live(c)) {
            self.claim_dirty.insert(c);
            self.make_dirty(c);
        }
        if let Some(p) = prev {
            self.claimer_near(p);
        }
        self.stab_structure(unit, label);
    }

    /// Replaces the text (operation and arguments) of a client node in place.
    pub fn replace(&mut self, node: NodeId, op: OpId, args: Vec<ValueId>) {
        span!(Edit);
        let s = self.slot_of(node);
        let args = self.values.list(args);
        if self.n(s).op == op && self.n(s).args == args {
            return;
        }
        self.reset_text(s, op, args);
    }

    /// Re-reads the static hooks of every live node `pred` selects (by its
    /// text), for hooks that depend on client state outside the node's text
    /// (e.g. a program-wide table of what each procedure may define).
    /// Linear in the live nodes; meant for rare, program-wide hint changes.
    ///
    /// # Panics
    ///
    /// If the engine holds more than `u32::MAX` node slots.
    pub fn refresh_where(&mut self, pred: &dyn Fn(OpId, &[ValueId], &Values) -> bool) {
        span!(Edit);
        // The hooks' answers for those texts may have changed.
        let stale: Vec<(OpId, ValueId)> = self
            .statics
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .copied()
            .collect::<Vec<_>>()
            .into_iter()
            .filter(|&(op, a)| pred(op, &self.args_vec(a), &self.values))
            .collect();
        let st = self.statics.get_mut().unwrap_or_else(PoisonError::into_inner);
        for k in stale {
            st.remove(&k);
        }
        let hit: Vec<Slot> = (0..self.nodes.len())
            .filter(|&i| {
                let n = &self.nodes[i];
                n.state != State::Free && !n.has(F_APP) && pred(n.op, &self.args_vec(n.args), &self.values)
            })
            .map(|i| self.slot_at(i))
            .collect();
        for s in hit {
            if self.live(s) {
                let (op, args) = (self.n(s).op, self.n(s).args);
                self.reset_text(s, op, args);
            }
        }
    }

    fn reset_text(&mut self, s: Slot, op: OpId, args: ValueId) {
        self.touch(s);
        let (unit, label) = (self.n(s).unit, self.n(s).label);
        self.dirty_reaching_claims(unit, label);
        if self.n(s).has(F_INPEND) {
            // Its pending fields may change with its text.
            self.pending_index(s, false);
            self.set_flag(s, F_INPEND, false);
        }
        self.nm(s).op = op;
        self.nm(s).args = args;
        let st = self.statics(op, args);
        let decl = st.decl.clone();
        let eff = st.eff;
        let claim_hook = st.claim;
        let hint = st.reads.is_some();
        let inert = self.n(s).state == State::Inert;
        let maybe = decl.is_none() && st.may_define != MayDefine::Any;
        self.set_flag(s, F_MAYBE, maybe);
        self.set_flag(s, F_HINT, hint);
        let predict = st.predict;
        self.set_flag(s, F_PREDICT, predict);
        self.stream_touch(self.n(s).unit, self.n(s).label);
        self.set_flag(s, F_CLAIMER, claim_hook != Claim::None);
        self.set_flag(s, F_CLAIM_DYN, claim_hook == Claim::Pending);
        self.set_flag(s, F_EFF_DYN, eff == Effect::Pending);
        if inert {
            // Text inside a claim: the claimer re-reads it.
            self.nm(s).effect = eff_code(eff);
            self.set_flag(s, F_DYN, decl.is_none());
            if let Some(c) = self.aux_of(s).and_then(|a| a.claimed_by) {
                self.claim_dirty.insert(c);
                self.make_dirty(c);
            }
        } else {
            match decl {
                Some(d) => {
                    self.set_flag(s, F_DYN, false);
                    let d: Vec<(Field, bool)> = d.into_iter().map(|(f, l)| (f, l == Locality::Global)).collect();
                    self.set_defset(s, &d);
                }
                None => self.set_flag(s, F_DYN, true),
            }
            let new_eff = if eff == Effect::Pending {
                self.n(s).effect
            } else {
                eff_code(eff)
            };
            if eff != Effect::Pending {
                self.ax(s).kind = eff_kind(eff);
            }
            self.change_effect(s, new_eff);
            self.make_dirty(s);
            self.sync_pending(s);
            if self.n(s).has(F_CLAIMER) || self.aux_of(s).is_some_and(|a| !a.claimed.is_empty()) {
                self.claim_dirty.insert(s);
            }
        }
        let prev = self
            .order
            .prev(self.units[self.n(s).unit as usize].root, self.n(s).label)
            .map(|(_, i)| i.slot);
        if let Some(p) = prev {
            self.claimer_near(p);
        }
        let (u, l) = (self.n(s).unit, self.n(s).label);
        self.stab_structure(u, l);
    }

    /// Sets the value a field has at the start of `unit`.
    pub fn set_env(&mut self, unit: UnitId, field: Field, value: ValueId) {
        self.set_env_slot(unit.0, field, value);
    }

    /// Supplies the environment for the next evaluation: the fields every
    /// text unit sees defined before its first node. Applied when the
    /// evaluation starts; a field that changed from the previous environment
    /// re-runs only its readers. Bodies of lexical applications do not see
    /// it (their results are memoized by their arguments).
    pub fn set_environment(&mut self, env: impl IntoIterator<Item = (Field, ValueId)>) {
        self.env_next = Some(env.into_iter().collect());
    }

    /// Declares environment fields absent for the next evaluation: a
    /// [`Ctx::use_env`] of one reads `None` instead of waiting. The set
    /// replaces the previous one, like the environment itself.
    pub fn set_environment_absent(&mut self, fields: impl IntoIterator<Item = Field>) {
        self.env_absent_next = Some(fields.into_iter().collect());
    }

    /// The environment of the last evaluation (or of the next, once
    /// supplied).
    pub fn environment(&self) -> impl Iterator<Item = (Field, ValueId)> + '_ {
        self.env_next
            .as_ref()
            .unwrap_or(&self.environment)
            .iter()
            .map(|(&f, &v)| (f, v))
    }

    fn apply_environment(&mut self) {
        let mut changed: Vec<Field> = Vec::new();
        if let Some(next) = self.env_next.take() {
            let old = std::mem::replace(&mut self.environment, next);
            changed.extend(
                old.iter()
                    .filter(|&(f, v)| self.environment.get(f) != Some(v))
                    .map(|(&f, _)| f)
                    .chain(self.environment.keys().filter(|f| !old.contains_key(f)).copied()),
            );
        }
        if let Some(next) = self.env_absent_next.take() {
            let old = std::mem::replace(&mut self.env_absent, next);
            changed.extend(old.symmetric_difference(&self.env_absent).copied());
        }
        if changed.is_empty() {
            return;
        }
        changed.sort_unstable();
        changed.dedup();
        // Nodes that waited for one of these fields run again.
        for &f in &changed {
            for x in self.env_waiters.remove(&f).unwrap_or_default() {
                if self.live(x) && self.n(x).state == State::Parked && self.n(x).on == NIL {
                    self.make_dirty(x);
                }
            }
        }
        for unit in 0..u32::try_from(self.units.len()).expect("unit count") {
            let u = &self.units[unit as usize];
            if !u.live || u.owner != NIL {
                continue;
            }
            for &f in &changed {
                if !self.units[unit as usize].env.contains_key(&f) {
                    self.dirty_field_users(site_key(unit, NIL), f, &Cause::Environment { field: f });
                }
            }
        }
    }

    /// The environment's value of `f` for a node of `unit`: `Err(())` when
    /// neither supplied nor declared absent (a body sees none).
    pub(crate) fn env_value(&self, unit: u32, f: Field) -> Result<Option<ValueId>, ()> {
        if self.units[unit as usize].owner != NIL {
            return Ok(None);
        }
        match self.environment.get(&f) {
            Some(&v) => Ok(Some(v)),
            None if self.env_absent.contains(&f) => Ok(None),
            None => Err(()),
        }
    }

    /// The value `f` has at the start of `unit`.
    pub(crate) fn start_value(&self, unit: u32, f: Field) -> Option<ValueId> {
        let u = &self.units[unit as usize];
        u.env
            .get(&f)
            .or_else(|| if u.owner == NIL { self.environment.get(&f) } else { None })
            .copied()
    }

    fn set_env_slot(&mut self, unit: u32, field: Field, value: ValueId) {
        let old = self.units[unit as usize].env.insert(field, value);
        if old != Some(value) {
            self.dirty_field_users(site_key(unit, NIL), field, &Cause::Environment { field });
        }
    }

    /// Marks `node` as a root: part of the program's output. Evaluation is
    /// eager (every node runs once its bindings are known); roots only
    /// matter for what the client reads back.
    pub fn add_root(&mut self, node: NodeId) {
        let s = self.slot_of(node);
        self.roots.insert(s);
    }

    pub fn remove_root(&mut self, node: NodeId) {
        if let Some(&s) = self.ids.get(&node) {
            self.roots.remove(&s);
        }
    }

    /// Declares `field` loop-back with its round-one value.
    pub fn declare_loopback(&mut self, field: Field, initial: ValueId) {
        self.loop_init.insert(field, initial);
        if self.loop_vals.insert(field, initial) != Some(initial) {
            self.dirty_loop_users(field);
        }
    }

    /// Declares `field` an accumulator: nodes contribute values
    /// ([`Ctx::contribute`]) and readers see the fold in program order.
    pub fn declare_accumulator(&mut self, field: Field, monoid: Arc<dyn Monoid>) {
        self.monoids.insert(field, monoid);
    }

    // ---------------------------------------------------------------- nodes

    fn set_flag(&mut self, s: Slot, f: u16, on: bool) {
        let n = self.nm(s);
        if on {
            n.flags |= f;
        } else {
            n.flags &= !f;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_node(
        &mut self,
        unit: u32,
        after: Option<Slot>,
        op: OpId,
        args: ValueId,
        parent: Slot,
        id: NodeId,
        provisional: bool,
    ) -> Slot {
        let s = self.create_raw(unit, after, None, op, args, parent, id, provisional);
        self.set_created_effect(s, true);
        self.bind_created(s, None);
        s
    }

    /// Phase 1 of creation: the record and its place in the order, with no
    /// scope effect and no definitions yet.
    #[allow(clippy::too_many_arguments)]
    fn create_raw(
        &mut self,
        unit: u32,
        after: Option<Slot>,
        label: Option<u64>,
        op: OpId,
        args: ValueId,
        parent: Slot,
        id: NodeId,
        provisional: bool,
    ) -> Slot {
        count!(Create);
        let st = self.statics(op, args);
        let (eff, claim_hook) = (st.eff, st.claim);
        let mut flags = 0;
        if st.decl.is_none() {
            flags |= F_DYN;
        }
        if eff == Effect::Pending {
            flags |= F_EFF_DYN;
        }
        if claim_hook != Claim::None {
            flags |= F_CLAIMER;
        }
        if claim_hook == Claim::Pending {
            flags |= F_CLAIM_DYN;
        }
        if st.reads.is_some() {
            flags |= F_HINT;
        }
        if st.predict {
            flags |= F_PREDICT;
        }
        if st.decl.is_none() && st.may_define != MayDefine::Any {
            flags |= F_MAYBE;
        }
        if provisional {
            flags |= F_PROV;
        }
        // A static effect goes into the order with the record: one
        // insertion instead of an insertion and an update.
        let effect = if eff == Effect::Pending {
            EFF_NONE
        } else {
            eff_code(eff)
        };
        let node = Node {
            id,
            args,
            output: ValueId::UNIT,
            label: 0,
            sig: 0,
            op,
            unit,
            parent,
            height: 0,
            floor: 0,
            epoch: 0,
            gen_: 0,
            aux: NO_AUX,
            on: NIL,
            state: State::Dirty,
            effect,
            bound: EFF_NONE,
            flags,
            uses: Box::new([]),
            defs: Box::new([]),
        };
        let s = if let Some(ix) = self.free_slots.pop() {
            let gen_ = self.nodes[ix as usize].gen_;
            self.nodes[ix as usize] = Node { gen_, ..node };
            Slot { ix, gen_ }
        } else {
            self.nodes.push(node);
            Slot {
                ix: u32::try_from(self.nodes.len() - 1).expect("too many nodes"),
                gen_: 0,
            }
        };
        assert!(self.ids.insert(id, s).is_none(), "node id collision");
        self.touched.insert(
            id,
            Snap {
                existed: false,
                sig: 0,
                args: ValueId::NONE,
                out: 0,
                sites: Vec::new(),
            },
        );
        if parent == NIL && self.units[unit as usize].owner != NIL && after.is_none() {
            // A body-unit child placed at the start.
        }
        let label = label.unwrap_or_else(|| self.alloc_label(unit, after));
        self.nm(s).label = label;
        self.stream_touch(unit, label);
        let item = self.oitem(s);
        self.nm(s).bound = item.eff;
        let root = self.units[unit as usize].root;
        self.units[unit as usize].root = self.order.insert(&OAug, root, label, item);
        s
    }

    /// Phase 2: the node's static scope effect (re-binding the region it
    /// changes unless `region` is false).
    /// (Without `region`, the caller re-binds and then calls
    /// `set_created_kind`.)
    fn set_created_effect(&mut self, s: Slot, region: bool) {
        let eff = self.statics(self.n(s).op, self.n(s).args).eff;
        if eff == Effect::Pending || eff == Effect::None {
            return;
        }
        if self.n(s).effect != eff_code(eff) {
            self.nm(s).effect = eff_code(eff);
            self.sync_order(s);
        }
        if region {
            self.effect_region_changed(s, EFF_NONE);
            self.set_kind(s, eff_kind(eff));
        }
    }

    fn set_created_kind(&mut self, s: Slot) {
        let eff = self.statics(self.n(s).op, self.n(s).args).eff;
        if eff != Effect::Pending && eff != Effect::None {
            self.set_kind(s, eff_kind(eff));
        }
    }

    /// Binds a run of freshly created siblings, with the heights the
    /// parallel phase computed for them (if any; one per node, in order).
    /// In-text siblings' definitions raise those heights.
    fn bind_fresh(&mut self, created: &[Slot], heights: &[u32]) {
        let mut in_text: FxHashMap<Field, u32> = FxHashMap::default();
        for (k, &c) in created.iter().enumerate() {
            let st = self.statics(self.n(c).op, self.n(c).args);
            let pre_h = heights.get(k).map(|&h| {
                let mut h = h;
                for f in st.reads.iter().flatten() {
                    if let Some(&d) = in_text.get(f) {
                        h = h.max(d + 1);
                    }
                }
                h
            });
            self.bind_created(c, pre_h);
            if let Some(d) = &st.decl {
                let h = self.n(c).height;
                for &(f, _) in d {
                    in_text.insert(self.canon(f), h);
                }
            }
        }
        self.structure_edit_run(created);
    }

    /// [`Self::structure_edit`] for a run of consecutive fresh siblings:
    /// the claims reaching into it are those reaching its first node (the
    /// claimers inside it are fresh, already marked), and the range readers
    /// over it are found with one query.
    fn structure_edit_run(&mut self, created: &[Slot]) {
        let (Some(&first), Some(&last)) = (created.first(), created.last()) else {
            return;
        };
        let n = self.n(first);
        let (unit, lo) = (n.unit, n.label);
        let hi = self.n(last).label;
        self.dirty_reaching_claims(unit, lo);
        if let Some((_, i)) = self.order.prev(self.units[unit as usize].root, lo) {
            let p = i.slot;
            self.claimer_near(p);
        }
        let u = &self.units[unit as usize];
        let from = lo.saturating_sub(u.max_span);
        let readers: Vec<Slot> = u
            .watches
            .range((from, Slot { ix: 0, gen_: 0 })..=(hi, NIL))
            .map(|((_, r), _)| *r)
            .filter(|&r| lo < self.n(r).label)
            .collect();
        for r in readers {
            self.make_dirty(r);
        }
    }

    /// Phase 3: definitions, pending structure, initial height, claims.
    fn bind_created(&mut self, s: Slot, height: Option<u32>) {
        let (unit, label) = (self.n(s).unit, self.n(s).label);
        let st = self.statics(self.n(s).op, self.n(s).args);
        let hints = st.reads.clone();
        if let Some(d) = &st.decl {
            for &(f, l) in d {
                let f = self.canon(f);
                self.add_def(s, f, l == Locality::Global);
            }
        }
        self.sync_pending(s);
        if let Some(h) = height {
            // Precomputed in the parallel phase.
            self.set_height(s, h);
        } else if let Some(h) = hints {
            // Order derived from the static structure: start above the
            // hinted inputs, so most nodes find their inputs ready.
            let mut height = 0;
            for f in h {
                let w = self.walk(unit, label, f);
                if w.site != NIL {
                    height = height.max(self.n(w.site).height + 1);
                }
                if w.pending != NIL {
                    height = height.max(self.n(w.pending).height + 1);
                }
            }
            self.set_height(s, height);
        }
        if self.n(s).has(F_CLAIMER) {
            self.claim_dirty.insert(s);
        }
        // Eager: every node runs once its bindings are known.
        self.enqueue(s);
    }

    fn create_app(&mut self, op: OpId, args: ValueId) -> Slot {
        self.apps_live += 1;
        let id = NodeId(hash_words(12, &[u64::from(op.0), args.0]));
        let body = self.alloc_unit(NIL);
        let mut flags = F_APP;
        if self.statics(op, args).reads.is_some() {
            flags |= F_HINT;
        }
        let node = Node {
            id,
            args,
            output: ValueId::UNIT,
            label: 0,
            sig: 0,
            op,
            unit: body,
            parent: NIL,
            height: 0,
            floor: 0,
            epoch: 0,
            gen_: 0,
            aux: NO_AUX,
            on: NIL,
            state: State::Dirty,
            effect: EFF_NONE,
            bound: EFF_NONE,
            flags,
            uses: Box::new([]),
            defs: Box::new([]),
        };
        let s = if let Some(ix) = self.free_slots.pop() {
            let gen_ = self.nodes[ix as usize].gen_;
            self.nodes[ix as usize] = Node { gen_, ..node };
            Slot { ix, gen_ }
        } else {
            self.nodes.push(node);
            Slot {
                ix: u32::try_from(self.nodes.len() - 1).expect("too many nodes"),
                gen_: 0,
            }
        };
        assert!(self.ids.insert(id, s).is_none(), "node id collision");
        self.touched.insert(
            id,
            Snap {
                existed: false,
                sig: 0,
                args: ValueId::NONE,
                out: 0,
                sites: Vec::new(),
            },
        );
        self.units[body as usize].owner = s;
        self.ax(s).body = Some(body);
        self.memo.insert((op, args), s);
        self.enqueue(s);
        s
    }

    /// Whether `s` opens a scope before itself (with `run` the effect its
    /// current run has set): its own reads are inside that scope.
    pub(crate) fn opens_before(&self, s: Slot, run: Option<u8>) -> bool {
        let e = run.unwrap_or_else(|| self.oitem(s).eff);
        e & EFF_BEFORE != 0 && eff_opens(e) > 0
    }

    fn oitem(&self, s: Slot) -> OItem {
        let n = self.n(s);
        let inert = n.state == State::Inert;
        let reach = if !inert && n.has(F_INPEND) && n.has(F_CLAIM_DYN) {
            self.aux_of(s)
                .map_or(u64::MAX, |a| a.reach1.checked_sub(1).unwrap_or(u64::MAX))
        } else {
            0
        };
        OItem {
            slot: s,
            // A pending effect keeps the one the bindings reflect until the
            // node is done (the pending-effect index makes readers wait).
            eff: if inert {
                EFF_NONE
            } else if n.has(F_EFF_DYN) && n.in_work() {
                n.bound
            } else {
                n.effect
            },
            pend: !inert && n.has(F_INPEND),
            work: n.in_work(),
            h: n.height,
            reach,
            hit: if self.claim_dirty.contains(&s) { 0 } else { reach },
            sreach: self
                .aux_of(s)
                .and_then(|a| a.stream)
                .map_or(0, |r| self.stream_labels(s, r).1),
        }
    }

    fn sync_order(&mut self, s: Slot) {
        let n = self.n(s);
        if n.has(F_APP) {
            return;
        }
        let (unit, label) = (n.unit, n.label);
        let item = self.oitem(s);
        let root = self.units[unit as usize].root;
        self.order.update(&OAug, root, label, &mut |i| *i = item);
    }

    fn subtree_end(&self, mut x: Slot) -> Slot {
        loop {
            let mut best = x;
            let mut best_l = self.n(x).label;
            if let Some(a) = self.aux_of(x) {
                if a.body.is_none()
                    && let Some(&c) = a.children.last()
                {
                    let e = self.subtree_end(c);
                    if self.n(e).label > best_l {
                        best = e;
                        best_l = self.n(e).label;
                    }
                }
                if let Some(&c) = a.claimed.last()
                    && self.n(c).label > best_l
                {
                    best = c;
                }
            }
            if best == x {
                return x;
            }
            x = best;
        }
    }

    // ------------------------------------------------------------- labels

    fn alloc_label(&mut self, unit: u32, after: Option<Slot>) -> u64 {
        self.alloc_labels(unit, after, 1)[0]
    }

    /// `k` labels for nodes to go, in order, right after `after` (the
    /// unit's start: `None`): spread over the gap, which is widened by one
    /// renumbering when it has no room for them.
    fn alloc_labels(&mut self, unit: u32, after: Option<Slot>, k: usize) -> Vec<u64> {
        let k = k.max(1) as u64;
        loop {
            let lo = after.map_or(0, |a| self.n(a).label);
            let root = self.units[unit as usize].root;
            let hi = self.order.next(root, lo).map_or(u64::MAX, |(k, _)| k);
            if hi == u64::MAX && (u64::MAX - lo) / GAP > k + 1 {
                return (1..=k).map(|i| lo + GAP * i).collect();
            }
            if hi - lo > k {
                let step = (hi - lo) / (k + 1);
                return (1..=k).map(|i| lo + step * i).collect();
            }
            self.relabel_around(unit, lo, k + 1);
        }
    }

    /// Renumbers the labels around `lo` so that at least `need` labels fit
    /// between any two neighbours there.
    fn relabel_around(&mut self, unit: u32, lo: u64, need: u64) {
        let mut w = 8usize;
        loop {
            let root = self.units[unit as usize].root;
            let mut before = Vec::new();
            let mut k = lo.saturating_add(1);
            while before.len() < w {
                match self.order.prev(root, k) {
                    Some((key, i)) => {
                        before.push(i.slot);
                        k = key;
                    }
                    None => break,
                }
            }
            before.reverse();
            let mut window = before;
            let mut k = lo;
            for _ in 0..w {
                match self.order.next(root, k) {
                    Some((key, i)) => {
                        window.push(i.slot);
                        k = key;
                    }
                    None => break,
                }
            }
            let first = self.n(window[0]).label;
            let last = self.n(*window.last().expect("non-empty")).label;
            let lo_b = self.order.prev(root, first).map_or(0, |(k, _)| k);
            let hi_b = self.order.next(root, last).map_or(u64::MAX, |(k, _)| k);
            let count = window.len() as u64;
            let gap = (hi_b - lo_b) / (count + 2);
            let whole = lo_b == 0 && hi_b == u64::MAX;
            if gap >= (2 * w as u64).max(need) || (whole && gap >= need.max(2)) {
                // Stream ranges summarised under the old labels: those of
                // claimers before the window reaching into it, and of the
                // window's own (attached before every label is final).
                let mut streams = self.stream_reaching(unit, first);
                streams.extend(
                    window
                        .iter()
                        .copied()
                        .filter(|&x| self.aux_of(x).is_some_and(|a| a.stream.is_some())),
                );
                for &x in &window {
                    self.detach_labels(x);
                }
                for (i, &x) in window.iter().enumerate() {
                    self.nm(x).label = lo_b + gap * (i as u64 + 1);
                    self.attach_labels(x);
                }
                for x in streams {
                    self.sync_order(x);
                }
                self.rebuild_watches(unit);
                self.relabels += 1;
                return;
            }
            assert!(!whole, "unit label space exhausted");
            w *= 2;
        }
    }

    fn detach_labels(&mut self, x: Slot) {
        let n = self.n(x);
        let (unit, label) = (n.unit, n.label);
        let defs: Vec<(Field, u32)> = n.defs.iter().map(|d| (d.field, d.depth)).collect();
        let uses: Vec<(u64, Field)> = n
            .uses
            .iter()
            .filter(|u| u.kind == K_FIELD)
            .map(|u| (site_key(unit, u.site), Field(ValueId(u.key))))
            .collect();
        let pend = n.has(F_INPEND);
        let root = self.units[unit as usize].root;
        let (r, _) = self.order.remove(&OAug, root, label);
        self.units[unit as usize].root = r;
        for (f, depth) in defs {
            self.ix_remove(unit, f, label, depth);
        }
        for (s, f) in uses {
            self.users.remove(&(s, f, label));
        }
        if pend {
            self.pending_index(x, false);
        }
        let contribs: Vec<Field> = self
            .aux_of(x)
            .map_or_else(Default::default, |a| a.contribs.iter().map(|c| c.0).collect());
        for f in contribs {
            self.acc_set(unit, f, label, None);
        }
    }

    fn attach_labels(&mut self, x: Slot) {
        let n = self.n(x);
        let (unit, label) = (n.unit, n.label);
        let defs: Vec<Field> = n.defs.iter().map(|d| d.field).collect();
        let uses: Vec<(u64, Field)> = n
            .uses
            .iter()
            .filter(|u| u.kind == K_FIELD)
            .map(|u| (site_key(unit, u.site), Field(ValueId(u.key))))
            .collect();
        let pend = n.has(F_INPEND);
        let item = self.oitem(x);
        self.nm(x).bound = item.eff;
        let root = self.units[unit as usize].root;
        self.units[unit as usize].root = self.order.insert(&OAug, root, label, item);
        for f in defs {
            self.ix_insert(unit, f, label, x);
        }
        for (s, f) in uses {
            self.users.insert((s, f, label), x);
        }
        if pend {
            self.pending_index(x, true);
        }
        let contribs: Vec<(Field, ValueId)> = self.node_contrib_folds(x);
        for (f, v) in contribs {
            self.acc_set(unit, f, label, Some(v));
        }
    }

    // ------------------------------------------------------------- binding

    fn def_global(&self, d: Slot, f: Field) -> bool {
        self.n(d).defs.iter().any(|x| x.field == f && x.global)
    }

    /// Whether the node at `label` lies after its own brackets.
    fn brackets_before(&self, root: u32, label: u64) -> bool {
        self.order.get(root, label).is_some_and(|i| i.eff & EFF_BEFORE != 0)
    }

    /// Whether the node at `label` defines `f` inside its own brackets:
    /// after them if they come before it; an opener's kind (and sequence
    /// marker) inside the scope it opens. Its other definitions lie
    /// outside: a plain opener defines in the enclosing scope.
    fn def_inside(&self, root: u32, label: u64, f: Field) -> bool {
        let opener_mark = (f == self.kind_field || self.is_seq_hidden(f))
            && self.order.get(root, label).is_some_and(|i| eff_opens(i.eff) > 0);
        opener_mark || self.brackets_before(root, label)
    }

    /// The scope depth at which the node at `label` defines `f`.
    fn def_depth(&self, root: u32, label: u64, f: Field) -> u32 {
        if let Ok(i) = self.run_depths.binary_search_by_key(&label, |r| r.0) {
            let (_, eff, before) = self.run_depths[i];
            let inside =
                ((f == self.kind_field || self.is_seq_hidden(f)) && eff_opens(eff) > 0) || eff & EFF_BEFORE != 0;
            return if inside {
                eff_opens(eff) + before.saturating_sub(eff_closes(eff))
            } else {
                before
            };
        }
        let inside = self.def_inside(root, label, f);
        self.order
            .fold(&OAug, root, 0, if inside { label + 1 } else { label })
            .o
    }

    fn ix_insert(&mut self, unit: u32, f: Field, label: u64, x: Slot) {
        self.defs_ix.insert((unit, f, label), x);
        if self.def_global(x, f) {
            self.global_ix.insert((unit, f, label), x);
        } else {
            let root = self.units[unit as usize].root;
            let depth = self.def_depth(root, label, f);
            self.local_ix.insert((unit, f, depth, label), x);
            for d in &mut self.nm(x).defs {
                if d.field == f {
                    d.depth = depth;
                }
            }
        }
    }

    /// The depth `x`'s definition of `f` is indexed at.
    fn indexed_depth(&self, x: Slot, f: Field) -> u32 {
        self.n(x).defs.iter().find(|d| d.field == f).map_or(0, |d| d.depth)
    }

    fn ix_remove(&mut self, unit: u32, f: Field, label: u64, depth: u32) {
        self.defs_ix.remove(&(unit, f, label));
        if self.global_ix.remove(&(unit, f, label)).is_none() {
            self.local_ix.remove(&(unit, f, depth, label));
        }
    }

    /// The reaching definition of `f` for a use at `label`, and the nearest
    /// pending node the walk crosses. A local definition reaches the use iff
    /// it lies directly in one of the scopes enclosing the use: one range
    /// query per enclosing level, never a scan over closed scopes.
    pub(crate) fn walk(&self, unit: u32, label: u64, f: Field) -> WalkResult {
        count!(Walk);
        let root = self.units[unit as usize].root;
        let mut local: Option<(u64, Slot, u32)> = None;
        // Brackets up to `bhi` (exclusive) enclose the use: a node whose
        // brackets come before it lies after them, except that the kind it
        // reads is that of the scope around its brackets (which its own
        // effect depends on).
        let mut bhi = if f != self.kind_field && self.brackets_before(root, label) {
            label + 1
        } else {
            label
        };
        let mut depth = self.order.fold(&OAug, root, 0, bhi).o;
        let mut hi = label;
        // Closes the node at `bhi` makes before it opens (a separator that
        // closes a scope and opens the next): they close that many scopes
        // around it, so the opener of the level outside it lies past them.
        let mut extra = 0;
        loop {
            count!(WalkLevel);
            let opener = if depth == 0 {
                None
            } else {
                self.order.search_back(&OAug, root, bhi, &|s: OSum| s.o > extra)
            };
            // The opener's own definitions inside its scope (its kind, or
            // all of them if it opens before itself) count at this level.
            let lo = opener.unwrap_or(0);
            if let Some((&(_, _, _, l), &d)) = self
                .local_ix
                .range((unit, f, depth, lo)..(unit, f, depth, hi))
                .next_back()
            {
                local = Some((l, d, depth));
                break;
            }
            match opener {
                Some(k) if depth > 0 => {
                    // The opener's definitions outside its scope count at
                    // the next level out.
                    extra = self.order.get(root, k).map_or(0, |i| eff_closes(i.eff));
                    bhi = k;
                    hi = (k + 1).min(hi);
                    depth -= 1;
                }
                _ => break,
            }
        }
        let global = self
            .global_ix
            .range((unit, f, 0)..(unit, f, label))
            .next_back()
            .map(|(&(_, _, l), &d)| (l, d));
        let (site, site_label) = match (local, global) {
            (Some((ll, ld, _)), Some((gl, gd))) => {
                if ll > gl {
                    (ld, ll)
                } else {
                    (gd, gl)
                }
            }
            (Some((ll, ld, _)), None) => (ld, ll),
            (None, Some((gl, gd))) => (gd, gl),
            (None, None) => (NIL, 0),
        };
        let all = self
            .pending
            .range((unit, site_label + 1)..(unit, label))
            .rev()
            .find(|&(_, &p)| self.pending_for(p, f))
            .map(|(&(_, l), &p)| (l, p));
        let eff = self.walk_effects(unit, f, site_label, label);
        let one = self
            .pending_f
            .range((unit, f, site_label + 1)..(unit, f, label))
            .next_back()
            .map(|(&(_, _, l), &p)| (l, p));
        let pre = self.walk_prefixes(unit, f, site_label, label);
        // A pending claim before the site that could extend over it may
        // swallow the definition.
        let swallow = if site == NIL {
            None
        } else {
            self.order
                .search_back(&OAug, root, site_label, &|s: OSum| s.maxr >= site_label)
                .map(|l| (l, self.order.get(root, l).map_or(NIL, |i| i.slot)))
        };
        let pending = [all, eff, one, pre, swallow]
            .into_iter()
            .flatten()
            .max_by_key(|x| x.0)
            .map_or(NIL, |x| x.1);
        WalkResult { site, pending }
    }

    /// The nearest pending node in `(site, label)` bounded to a prefix of
    /// `f`'s name.
    fn walk_prefixes(&self, unit: u32, f: Field, site: u64, label: u64) -> Option<(u64, Slot)> {
        if self.pending_p_lens.is_empty() {
            return None;
        }
        let v = self.values.get(self.client_name(f).0);
        let Value::Str(name) = &*v else { return None };
        let name = name.as_bytes();
        let mut best: Option<(u64, Slot)> = None;
        for &len in self.pending_p_lens.keys() {
            if len > name.len() {
                break;
            }
            let h = hash_bytes(PREFIX_SEED, &name[..len]);
            if let Some((&(_, _, l), &p)) = self.pending_p.range((unit, h, site + 1)..(unit, h, label)).next_back()
                && best.is_none_or(|b| l > b.0)
            {
                best = Some((l, p));
            }
        }
        best
    }

    /// Whether pending node `p` (an unbounded definition set) could define
    /// `f`.
    fn pending_for(&self, p: Slot, f: Field) -> bool {
        let n = self.n(p);
        let av = self.args_vec(n.args);
        self.op(n.op).may_define_field(&av, &self.values, self.client_name(f))
    }

    /// The nearest pending scope effect in `(site, label)` that could change
    /// the binding of `f` at `label` (see `effect_matters`). None can unless
    /// a definition of `f` lies in that range or a close is ignored there;
    /// effects sharing an outermost scope are checked once.
    fn walk_effects(&self, unit: u32, f: Field, site: u64, label: u64) -> Option<(u64, Slot)> {
        let (&(_, first), &first_s) = self.pending_eff.range((unit, site + 1)..(unit, label)).next_back()?;
        if f == self.kind_field {
            // A pending effect may open a scope, defining a nearer kind.
            return Some((first, first_s));
        }
        let root = self.units[unit as usize].root;
        // The common case: no other definition and no close between the
        // site and the use, so every pending effect between them lies in
        // the site's top-level scope (if it has one), where it may hide it.
        if self
            .defs_ix
            .range((unit, f, site + 1)..(unit, f, label))
            .next()
            .is_none()
            && self.order.fold(&OAug, root, site + 1, label).c == 0
        {
            let in_scope = site != 0 && self.def_depth(root, site, f) > 0;
            return in_scope.then_some((first, first_s));
        }
        let ignored = |x: u64| self.order.fold(&OAug, root, 0, x).c;
        // A close ignored at depth 0 after a pending effect shifts every
        // depth up to it: any pending effect before the last such close in
        // the range counts.
        let target = ignored(label);
        if target > ignored(site + 1)
            && let Some(c) = self.order.search_fwd(&OAug, root, 0, &|s: OSum| s.c >= target)
            && let Some((&(_, p), &s)) = self
                .pending_eff
                .range((unit, site + 1)..(unit, c.min(label)))
                .next_back()
        {
            return Some((p, s));
        }
        // Otherwise a pending effect matters only together with a
        // definition of `f` in its top-level scope (the site itself counts:
        // a pending close may hide it): visit the pending effects one
        // top-level scope at a time.
        let mut hi = label;
        loop {
            if hi <= site + 1 {
                return None;
            }
            let (&(_, p), &s) = self.pending_eff.range((unit, site + 1)..(unit, hi)).next_back()?;
            match self.effect_scope(unit, p, label, f) {
                Ok(()) => return Some((p, s)),
                Err(open) if open == p => {
                    // `p` lies at depth 0, as does everything back to the
                    // close of the previous top-level scope: skip to it.
                    hi = self
                        .order
                        .search_back(&OAug, root, p, &|s: OSum| s.c >= 1)
                        .map_or(0, |k| k + 1);
                }
                Err(open) => hi = open.min(p),
            }
        }
    }

    /// Whether a scope effect still unknown at `p` could change the binding
    /// of `f` for a later use at `label`. Unmatched closes are ignored only at
    /// depth 0, so extra brackets at `p` re-match brackets only up to the end
    /// of `p`'s outermost enclosing scope `T` (an extra open leaves `T` open,
    /// an extra close makes `T`'s own close the ignored one), plus wherever a
    /// close was ignored at depth 0 after `p` (the caller has ruled that
    /// out). Only definitions of `f` in `T` can change visibility then;
    /// elsewhere the shift is uniform.
    /// `Err(open)`: it cannot, nor can any effect in `(open, p)`.
    fn effect_scope(&self, unit: u32, p: u64, label: u64, f: Field) -> Result<(), u64> {
        let root = self.units[unit as usize].root;
        let depth = self.order.fold(&OAug, root, 0, p).o;
        if depth == 0 {
            return Err(p);
        }
        let open = self
            .order
            .search_back(&OAug, root, p, &|s: OSum| s.o >= depth)
            .unwrap_or(0);
        let close = self
            .order
            .search_fwd(&OAug, root, p, &|s: OSum| s.c >= depth)
            .unwrap_or(u64::MAX);
        // An opener's start-of-sequence marker lies inside its scope.
        let from = if self.is_seq_hidden(f) { open } else { open + 1 };
        if self
            .defs_ix
            .range((unit, f, from)..(unit, f, close.min(label)))
            .next()
            .is_some()
        {
            Ok(())
        } else {
            Err(open)
        }
    }

    /// Where `s` is indexed while pending. A pending claim alone is not
    /// indexed: only the swallow check in `walk` can make a use wait on it.
    fn pend_ix(&self, s: Slot) -> PendIx {
        let n = self.n(s);
        let bounded = n.has(F_DYN) && n.has(F_MAYBE);
        let bound = if bounded {
            self.statics(n.op, n.args).may_define.clone()
        } else {
            MayDefine::Nothing
        };
        PendIx {
            unbounded: n.has(F_DYN) && !bounded,
            effect: n.has(F_EFF_DYN),
            bound,
        }
    }

    fn pending_index(&mut self, s: Slot, insert: bool) {
        let (unit, label) = (self.n(s).unit, self.n(s).label);
        // Removal uses what it was indexed under: the hook's answer may have
        // changed since (see `refresh_where`).
        let ix = if insert {
            let ix = self.pend_ix(s);
            self.ax(s).pend_fields = Some(ix.clone());
            ix
        } else {
            match self.aux_of(s).and_then(|a| a.pend_fields.clone()) {
                Some(ix) => ix,
                None => return,
            }
        };
        for (on, map) in [(ix.unbounded, &mut self.pending), (ix.effect, &mut self.pending_eff)] {
            if on {
                if insert {
                    map.insert((unit, label), s);
                } else {
                    map.remove(&(unit, label));
                }
            }
        }
        match ix.bound {
            MayDefine::Nothing | MayDefine::Any => {}
            MayDefine::Prefixes(ps) => {
                for p in ps {
                    let key = (unit, hash_bytes(PREFIX_SEED, p.as_bytes()), label);
                    let c = self.pending_p_lens.entry(p.len()).or_insert(0);
                    if insert {
                        *c += 1;
                        self.pending_p.insert(key, s);
                    } else {
                        *c -= 1;
                        if *c == 0 {
                            self.pending_p_lens.remove(&p.len());
                        }
                        self.pending_p.remove(&key);
                    }
                }
            }
            MayDefine::Names(fs) => {
                for f in fs {
                    let f = self.canon(f);
                    if insert {
                        self.pending_f.insert((unit, f, label), s);
                    } else {
                        self.pending_f.remove(&(unit, f, label));
                    }
                }
            }
        }
    }

    fn sync_pending(&mut self, s: Slot) {
        let n = self.n(s);
        let want = n.in_work()
            && n.state != State::Inert
            && !n.has(F_APP)
            && (n.has(F_DYN) || n.has(F_EFF_DYN) || n.has(F_CLAIM_DYN));
        if want == n.has(F_INPEND) {
            return;
        }
        self.pending_index(s, want);
        self.set_flag(s, F_INPEND, want);
        self.sync_order(s);
    }

    /// Makes `x`'s registered definitions equal `new`, re-binding uses.
    /// Sets the client definitions of `x` (its scope kind is kept).
    fn set_defset(&mut self, x: Slot, new: &[(Field, bool)]) {
        let old: Vec<(Field, bool)> = self
            .n(x)
            .defs
            .iter()
            .filter(|d| self.is_client_def(x, d.field) && d.value != self.seq_start)
            .map(|d| (d.field, d.global))
            .collect();
        let new: Vec<(Field, bool)> = new
            .iter()
            .map(|&(f, g)| (self.canon(f), g))
            .filter(|d| self.is_client_def(x, d.0))
            .collect();
        let new = &new[..];
        for &(f, g) in &old {
            if !new.contains(&(f, g)) {
                self.remove_def(x, f);
            }
        }
        for &(f, g) in new {
            if !old.contains(&(f, g)) {
                self.add_def(x, f, g);
            }
        }
    }

    fn move_user(&mut self, from: u64, to: u64, f: Field, label: u64, u: Slot, to_slot: Slot) {
        count!(MoveUser);
        self.users.remove(&(from, f, label));
        self.touch(u);
        self.users.insert((to, f, label), u);
        let unit = self.n(u).unit;
        let mut old_site = NIL;
        let n = self.nm(u);
        let mut uses = std::mem::take(&mut n.uses).into_vec();
        for us in &mut uses {
            if (us.kind == K_FIELD || us.kind == K_CHAIN) && us.key == f.0.0 && site_key(unit, us.site) == from {
                old_site = us.site;
                us.site = to_slot;
            }
        }
        n.uses = uses.into_boxed_slice();
        if self.tracing > 0 {
            let id = |e: &Engine, s: Slot| (s != NIL && e.live(s)).then(|| e.n(s).id);
            let (from, to) = (id(self, old_site), id(self, to_slot));
            self.note(u, Cause::Rebind { field: f, from, to });
        }
        let st = self.n(u).state;
        let only_rebind = st == State::Done || (st == State::Dirty && self.n(u).has(F_REBIND));
        self.make_dirty(u);
        if only_rebind && self.n(u).has(F_RAN) && self.n(u).state == State::Dirty {
            self.set_flag(u, F_REBIND, true);
        }
    }

    /// Registers a definition of `f` at `x` and moves exactly the uses that
    /// now reach it (a range split of the previous definitions' users).
    fn add_def(&mut self, x: Slot, f: Field, global: bool) {
        let n = self.n(x);
        let (unit, lx) = (n.unit, n.label);
        if n.has(F_APP) {
            return;
        }
        {
            let n = self.nm(x);
            let mut defs = std::mem::take(&mut n.defs).into_vec();
            // Re-registering keeps the value (a marker an opener already
            // holds; a run's value when only the locality changes).
            let value = defs.iter().find(|d| d.field == f).map_or(ValueId::NONE, |d| d.value);
            defs.retain(|d| d.field != f);
            defs.push(Def {
                field: f,
                value,
                global,
                depth: 0,
            });
            n.defs = defs.into_boxed_slice();
        }
        if self.fresh_scope && !global {
            self.ix_insert(unit, f, lx, x);
            self.stream_touch(unit, lx);
            return;
        }
        let root = self.units[unit as usize].root;
        // (previous site, users in (lx, end)) pairs.
        let mut prev: Vec<(Slot, u64)> = Vec::new();
        if global {
            // Every use after `lx` bound to the latest global before it
            // (wherever that lies: possibly inside a scope the walk below
            // steps over) now reaches `x`.
            if let Some((_, &g)) = self.global_ix.range((unit, f, 0)..(unit, f, lx)).next_back() {
                prev.push((g, u64::MAX));
            }
            let mut hi = lx;
            let mut last_rel: i64 = -1;
            loop {
                let Some((&(_, _, ld), &d)) = self.defs_ix.range((unit, f, 0)..(unit, f, hi)).next_back() else {
                    prev.push((NIL, u64::MAX));
                    break;
                };
                if self.def_global(d, f) {
                    if !prev.iter().any(|p| p.0 == d) {
                        prev.push((d, u64::MAX));
                    }
                    break;
                }
                let depth = self.def_depth(root, ld, f);
                let seg = self.order.fold(&OAug, root, ld, lx);
                if depth == 0 || seg.c == 0 {
                    let rel = i64::from(seg.o);
                    if rel > last_rel {
                        prev.push((d, u64::MAX));
                        last_rel = rel;
                    }
                    if depth == 0 {
                        break;
                    }
                }
                if let Some(k) = self.order.search_back(&OAug, root, ld, &|s: OSum| s.o >= 1) {
                    hi = k + 1;
                } else {
                    prev.push((NIL, u64::MAX));
                    break;
                }
            }
        } else {
            let site = self.walk(unit, lx, f).site;
            prev.push((site, self.local_end(root, lx, f)));
        }
        self.ix_insert(unit, f, lx, x);
        self.stream_touch(unit, lx);
        for (site, end) in prev {
            let from = site_key(unit, site);
            let movers: Vec<(u64, Slot)> = self
                .users
                .range((from, f, lx + 1)..(from, f, end))
                .map(|(&(_, _, l), &u)| (l, u))
                .collect();
            for (l, u) in movers {
                self.move_user(from, x.pack(), f, l, u, x);
                self.report.rebinds += 0;
            }
        }
    }

    /// The brackets around `s` changed: re-binds its definitions and uses.
    fn rescope_node(&mut self, unit: u32, s: Slot) {
        // A global's reach (from it to the next global, at any depth)
        // does not depend on the brackets: only local definitions
        // re-register. Re-adding a local takes from a global exactly the
        // readers that now reach the local.
        let (plain, special): (Vec<Field>, Vec<Field>) = self
            .n(s)
            .defs
            .iter()
            .filter(|d| !d.global)
            .map(|d| d.field)
            .partition(|&f| !self.is_hidden(f));
        for f in plain {
            self.rescope_def(s, f);
        }
        let vals: Vec<(Field, ValueId)> = self.n(s).defs.iter().map(|d| (d.field, d.value)).collect();
        for &f in &special {
            self.remove_def(s, f);
        }
        for &f in &special {
            self.add_def(s, f, false);
        }
        // Restore the values (re-registration resets them).
        let n = self.nm(s);
        for d in &mut n.defs {
            if let Some(v) = vals.iter().find(|v| v.0 == d.field) {
                d.value = v.1;
            }
        }
        let uses: Vec<Use> = self.n(s).uses.to_vec();
        let label = self.n(s).label;
        let streams = self.aux_of(s).is_some_and(|a| a.stream.is_some());
        // The previous link of each contribution chain read.
        let mut link: Vec<(u64, Slot)> = Vec::new();
        for u in uses {
            if u.kind == K_CHAIN {
                // A deeper link is read at the previous link's position:
                // the run is redone only if one moved, or now crosses
                // pending structure.
                let prev = link.iter().rev().find(|l| l.0 == u.key).map(|l| l.1);
                let same = !streams
                    && prev.is_some_and(|p| {
                        p != NIL && self.live(p) && {
                            let w = self.walk(unit, self.n(p).label, Field(ValueId(u.key)));
                            w.site == u.site && w.pending == NIL
                        }
                    });
                if !same {
                    self.make_dirty(s);
                }
                link.push((u.key, u.site));
                continue;
            }
            if u.kind != K_FIELD {
                continue;
            }
            let f = Field(ValueId(u.key));
            let w = self.walk(unit, label, f);
            if w.site != u.site {
                self.move_user(site_key(unit, u.site), site_key(unit, w.site), f, label, s, w.site);
            }
            link.push((u.key, w.site));
        }
    }

    /// Where the reach of a local definition of `f` at `lx` ends
    /// (exclusive): the close of its scope.
    fn local_end(&self, root: u32, lx: u64, f: Field) -> u64 {
        if self.def_depth(root, lx, f) == 0 {
            return u64::MAX;
        }
        // The first unmatched close after the definition's brackets: the
        // node's own brackets count unless the definition lies inside
        // them (then they are matched, or close the scope it is in).
        let after = if self.def_inside(root, lx, f) {
            lx
        } else {
            lx.saturating_sub(1)
        };
        // The closing node's own reads are inside the scope, unless its
        // brackets come before it (its kind read always is).
        self.order
            .search_fwd(&OAug, root, after, &|s: OSum| s.c >= 1)
            .map_or(u64::MAX, |e| {
                if f == self.kind_field || !self.brackets_before(root, e) {
                    e + 1
                } else {
                    e
                }
            })
    }

    /// The brackets around `x`'s local definition of `f` changed: re-binds
    /// exactly the uses whose reaching definition changed. Uses can leave
    /// only past the (new) end of its scope; uses can arrive only from the
    /// definition that reaches `x` itself. Unlike removing and adding the
    /// definition, uses that stay are not touched.
    fn rescope_def(&mut self, x: Slot, f: Field) {
        let (unit, lx) = (self.n(x).unit, self.n(x).label);
        let root = self.units[unit as usize].root;
        // The index is keyed by depth.
        let depth = self.indexed_depth(x, f);
        self.ix_remove(unit, f, lx, depth);
        self.ix_insert(unit, f, lx, x);
        let end = self.local_end(root, lx, f);
        let from = x.pack();
        if end < u64::MAX {
            let leavers: Vec<(u64, Slot)> = self
                .users
                .range((from, f, end)..=(from, f, u64::MAX))
                .map(|(&(_, _, l), &u)| (l, u))
                .collect();
            for (l, u) in leavers {
                let site = self.walk(unit, l, f).site;
                self.move_user(from, site_key(unit, site), f, l, u, site);
            }
        }
        let site = self.walk(unit, lx, f).site;
        let prev = site_key(unit, site);
        let movers: Vec<(u64, Slot)> = self
            .users
            .range((prev, f, lx + 1)..(prev, f, end))
            .map(|(&(_, _, l), &u)| (l, u))
            .collect();
        for (l, u) in movers {
            self.move_user(prev, from, f, l, u, x);
        }
    }

    /// Unregisters `x`'s definition of `f`; its uses re-bind.
    fn remove_def(&mut self, x: Slot, f: Field) {
        let n = self.n(x);
        let (unit, label) = (n.unit, n.label);
        let depth = self.indexed_depth(x, f);
        {
            let n = self.nm(x);
            let mut defs = std::mem::take(&mut n.defs).into_vec();
            defs.retain(|d| d.field != f);
            n.defs = defs.into_boxed_slice();
        }
        if self.n(x).has(F_APP) {
            return;
        }
        self.ix_remove(unit, f, label, depth);
        self.stream_touch(unit, label);
        let from = x.pack();
        let users: Vec<(u64, Slot)> = self
            .users
            .range((from, f, 0)..=(from, f, u64::MAX))
            .map(|(&(_, _, l), &u)| (l, u))
            .collect();
        for (l, u) in users {
            let site = self.walk(unit, l, f).site;
            self.move_user(from, site_key(unit, site), f, l, u, site);
        }
    }

    /// A node's scope effect changed (from `old` to its current effect, or
    /// its pending status changed): re-bind everything whose visibility may
    /// have changed, i.e. the region from it to where depth returns to 0.
    fn effect_region_changed(&mut self, p: Slot, old: u8) {
        span!(Region);
        let e = self.oitem(p).eff;
        self.nm(p).bound = e;
        let n = self.n(p);
        let (unit, lp) = (n.unit, n.label);
        self.stream_touch(unit, lp);
        let root = self.units[unit as usize].root;
        let before = self.order.fold(&OAug, root, 0, lp);
        let depth_after = |eff: u8| {
            OAug.combine(
                before,
                OAug.leaf(&OItem {
                    slot: p,
                    eff,
                    pend: false,
                    work: false,
                    h: 0,
                    reach: 0,
                    hit: 0,
                    sreach: 0,
                }),
            )
            .o
        };
        let d_old = depth_after(old);
        let d_new = depth_after(self.oitem(p).eff);
        let reach = |d: u32| {
            if d == 0 {
                lp
            } else {
                self.order
                    .search_fwd(&OAug, root, lp, &|s: OSum| s.c >= d)
                    .unwrap_or(u64::MAX)
            }
        };
        let end = reach(d_old).max(reach(d_new));
        // A node after its own brackets is inside the region.
        let before_p = (old | self.oitem(p).eff) & EFF_BEFORE != 0;
        if end == lp && !before_p {
            return;
        }
        // p's own definitions are inside the scope it opens: their reach
        // changes with its brackets, so they re-register too.
        let mut region = Vec::new();
        self.order.range(root, lp, end.max(lp), &mut region, usize::MAX);
        self.report.scope_region += region.len() as u64;
        count!(RegionNode, region.len());
        for (_, s) in region {
            if !self.live(s) || self.n(s).state == State::Inert {
                continue;
            }
            self.rescope_node(unit, s);
        }
    }

    fn change_effect(&mut self, s: Slot, new: u8) {
        let old = self.oitem(s).eff;
        self.nm(s).effect = new;
        self.sync_order(s);
        if self.oitem(s).eff != old {
            self.effect_region_changed(s, old);
        }
        self.sync_kind(s);
    }

    /// Sets the kind of the scope `s` opens (from its effect).
    fn set_kind(&mut self, s: Slot, kind: Option<ValueId>) {
        if self.aux_of(s).and_then(|a| a.kind) != kind {
            self.ax(s).kind = kind;
        }
        self.sync_kind(s);
    }

    /// An opener defines the kind of its scope inside it (a local
    /// definition of `kind_field`, read by [`Ctx::scope_kind`]).
    fn sync_kind(&mut self, s: Slot) {
        let opens = eff_opens(self.oitem(s).eff) > 0;
        let kind = self.aux_of(s).and_then(|a| a.kind).unwrap_or(ValueId::UNIT);
        let start = self.seq_start;
        let mut marks = vec![(self.kind_field, kind)];
        marks.extend(self.seqs.iter().map(|&(_, h)| (h, start)));
        for (f, v) in marks {
            let cur = self.n(s).defs.iter().find(|d| d.field == f).map(|d| d.value);
            if !opens {
                // Only the markers go; a contribution stays, and a declared
                // one (shadowed while this node opened) is registered.
                let marker = cur.is_some() && (f == self.kind_field || cur == Some(start));
                if marker {
                    self.remove_def(s, f);
                }
                if marker && f != self.kind_field && self.declares(s, f) {
                    self.add_def(s, f, false);
                    self.make_dirty(s);
                }
                continue;
            }
            if cur.is_none() {
                self.add_def(s, f, false);
            }
            if self.n(s).def_value(f) != Some(v) {
                for d in &mut self.nm(s).defs {
                    if d.field == f {
                        d.value = v;
                    }
                }
                let cause = Cause::Changed {
                    field: f,
                    site: Some(self.n(s).id),
                };
                self.dirty_field_users(s.pack(), f, &cause);
            }
        }
    }

    // --------------------------------------------------------------- claims

    /// After an edit next to `p`: the claimer whose range may now change.
    fn claimer_near(&mut self, p: Slot) {
        if !self.live(p) {
            return;
        }
        if let Some(c) = self.aux_of(p).and_then(|a| a.claimed_by) {
            self.claim_dirty.insert(c);
            self.make_dirty(c);
        } else if self.n(p).has(F_CLAIMER) {
            self.claim_dirty.insert(p);
        }
    }

    /// Pending claims whose syntactic bound covers `label` recompute it.
    fn dirty_reaching_claims(&mut self, unit: u32, label: u64) {
        let root = self.units[unit as usize].root;
        let mut before = label;
        let mut hit = Vec::new();
        if self.order.sum(&OAug, root).maxhit < label {
            return;
        }
        while let Some(k) = self
            .order
            .search_back(&OAug, root, before, &|s: OSum| s.maxhit >= label)
        {
            if let Some(i) = self.order.get(root, k) {
                hit.push(i.slot);
            }
            before = k;
        }
        // Their bounds and claims are recomputed with the next flush, once
        // for all the edits before it: a hit claim is not hit again until
        // then (its hit reach is 0).
        for c in hit {
            self.claim_dirty.insert(c);
            self.sync_order(c);
        }
    }

    fn structure_edit(&mut self, s: Slot) {
        let n = self.n(s);
        let (unit, label) = (n.unit, n.label);
        self.dirty_reaching_claims(unit, label);
        if let Some((_, i)) = self.order.prev(self.units[unit as usize].root, label) {
            let p = i.slot;
            self.claimer_near(p);
        }
        self.stab_structure(unit, label);
    }

    pub(crate) fn following(&self, s: Slot) -> Following<'_> {
        let n = self.n(s);
        let start = self
            .aux_of(s)
            .and_then(|a| a.claimed.last().copied())
            .map_or(n.label, |_| n.label);
        Following {
            eng: self,
            unit: n.unit,
            label: start,
            parent: n.parent,
            slots: Vec::new(),
        }
    }

    fn flush_claims(&mut self) {
        span!(FlushClaims);
        while !self.claim_dirty.is_empty() {
            let dirty: Vec<Slot> = self.claim_dirty.drain().collect();
            let mut batch: Vec<(u32, u64, Slot)> = dirty
                .into_iter()
                .filter(|&s| self.live(s))
                .map(|s| (self.n(s).unit, self.n(s).label, s))
                .collect();
            batch.sort_unstable();
            for (_, _, c) in batch {
                if self.live(c) {
                    self.recompute_claim(c);
                    // Reachable by structure edits again.
                    self.sync_order(c);
                }
            }
        }
    }

    fn recompute_claim(&mut self, c: Slot) {
        count!(ClaimRecompute);
        span!(Claim);
        if self.n(c).state == State::Inert || !self.n(c).has(F_CLAIMER) {
            self.set_claim(c, &[]);
            return;
        }
        if self.n(c).has(F_CLAIM_DYN) {
            // Pending: the run decides; keep the current claim as prediction.
            // Its syntactic bound decides which bindings it may affect.
            self.refresh_reach(c);
            return;
        }
        let av = self.args_vec(self.n(c).args);
        let opr = self.op(self.n(c).op).clone();
        let mut it = self.following(c);
        let claim = opr.claim(&av, &self.values, &mut it);
        let k = match claim {
            Claim::Count(k) => k,
            _ => 0,
        };
        self.claim_count(c, k);
    }

    /// Recomputes a pending claim's syntactic reach.
    fn refresh_reach(&mut self, c: Slot) {
        {
            let av = self.args_vec(self.n(c).args);
            let opr = self.op(self.n(c).op).clone();
            let mut it = self.following(c);
            let reach = match opr.claim_limit(&av, &self.values, &mut it) {
                Some(k) => {
                    // Up to and including the sibling that bounds it, so an
                    // edit to the bound itself is seen.
                    let mut it = self.following(c);
                    for _ in 0..=k {
                        if it.next().is_none() {
                            break;
                        }
                    }
                    it.slots.last().map_or(self.n(c).label, |&l| self.n(l).label)
                }
                None => u64::MAX - 1,
            };
            if self.aux_of(c).is_none_or(|a| a.reach1 != reach + 1) {
                self.ax(c).reach1 = reach + 1;
                self.sync_order(c);
            }
        }
    }

    /// Claims the first `k` following siblings of `c` (a run's decision).
    fn claim_count(&mut self, c: Slot, k: usize) {
        let mut it = self.following(c);
        for _ in 0..k {
            if it.next().is_none() {
                break;
            }
        }
        let slots = it.slots;
        self.set_claim(c, &slots);
    }

    fn set_claim(&mut self, c: Slot, new: &[Slot]) {
        let old = self.aux_of(c).map_or_else(Default::default, |a| a.claimed.clone());
        if old == new {
            return;
        }
        self.touch(c);
        if !old.is_empty() || !new.is_empty() {
            self.ax(c).claimed = new.to_vec();
        }
        for &s in &old {
            if !new.contains(&s) && self.live(s) && self.aux_of(s).and_then(|a| a.claimed_by) == Some(c) {
                self.activate(s);
            }
        }
        for &s in new {
            if !old.contains(&s) {
                if let Some(other) = self.aux_of(s).and_then(|a| a.claimed_by)
                    && other != c
                    && self.live(other)
                {
                    let mut l = self.aux_of(other).map_or_else(Default::default, |a| a.claimed.clone());
                    l.retain(|&x| x != s);
                    self.ax(other).claimed = l;
                    self.make_dirty(other);
                }
                self.make_inert(s, c);
            }
        }
        self.make_dirty(c);
    }

    /// Nodes parked on `x` (by a wait, a gate or a needed child) run again:
    /// `x` is going away, so it will never finish for them.
    fn release_waiters(&mut self, x: Slot, waiters: &[Slot]) {
        for &w in waiters {
            if self.live(w) && w != x && self.n(w).on == x {
                self.nm(w).on = NIL;
                if self.n(w).state == State::Parked {
                    if self.tracing > 0 {
                        self.note(w, Cause::Woken { by: self.n(x).id });
                    }
                    self.make_dirty_quiet(w);
                }
            }
        }
    }

    fn make_inert(&mut self, s: Slot, by: Slot) {
        if self.n(s).state == State::Inert {
            self.ax(s).claimed_by = Some(by);
            return;
        }
        self.touch(s);
        let mine = self.aux_of(s).map_or_else(Default::default, |a| a.claimed.clone());
        if !mine.is_empty() {
            self.ax(s).claimed.clear();
            for x in mine {
                if self.live(x) && self.aux_of(x).and_then(|a| a.claimed_by) == Some(s) {
                    self.activate(x);
                }
            }
        }
        let kids = self.aux_of(s).map_or_else(Default::default, |a| a.children.clone());
        if !kids.is_empty() {
            self.ax(s).children.clear();
            self.free_span(&kids, false);
        }
        let defs: Vec<Field> = self.n(s).defs.iter().map(|d| d.field).collect();
        for f in defs {
            self.remove_def(s, f);
        }
        self.set_uses(s, Vec::new());
        self.clear_contribs(s);
        // Not a sibling for anyone now: a reader anchored on it re-anchors.
        self.unanchor_watches(s);
        self.set_publishes(s, Vec::new());
        self.drop_watch(s);
        let readers = self.aux_of(s).map_or_else(Default::default, |a| a.out_readers.clone());
        for r in readers {
            self.make_dirty(r);
        }
        self.leave_spec(s);
        self.dequeue(s);
        self.refute_assumers(s);
        let waiters = std::mem::take(&mut self.ax(s).waiters);
        self.release_waiters(s, &waiters);
        let old_eff = self.oitem(s).eff;
        self.nm(s).state = State::Inert;
        self.nm(s).epoch = self.n(s).epoch.wrapping_add(1);
        self.sync_pending(s);
        self.sync_order(s);
        if old_eff != EFF_NONE {
            self.effect_region_changed(s, old_eff);
        }
        self.ax(s).claimed_by = Some(by);
    }

    fn activate(&mut self, s: Slot) {
        self.touch(s);
        self.ax(s).claimed_by = None;
        self.nm(s).state = State::Dirty;
        self.nm(s).epoch = self.n(s).epoch.wrapping_add(1);
        // A sibling again: a reader whose range spans it may anchor on it.
        let (unit, label) = (self.n(s).unit, self.n(s).label);
        self.stab_structure(unit, label);
        let st = self.statics(self.n(s).op, self.n(s).args);
        if let Some(d) = &st.decl {
            for &(f, l) in d {
                let f = self.canon(f);
                self.add_def(s, f, l == Locality::Global);
            }
        }
        let eff = st.eff;
        self.nm(s).effect = if eff == Effect::Pending {
            EFF_NONE
        } else {
            eff_code(eff)
        };
        self.sync_pending(s);
        self.sync_order(s);
        if self.oitem(s).eff != EFF_NONE {
            self.effect_region_changed(s, EFF_NONE);
        }
        if eff != Effect::Pending {
            self.set_kind(s, eff_kind(eff));
        }
        if self.n(s).has(F_CLAIMER) {
            self.claim_dirty.insert(s);
        }
        self.enqueue(s);
    }

    // ------------------------------------------------------------ work state

    fn enqueue(&mut self, x: Slot) {
        let n = self.n(x);
        if n.has(F_PROV) {
            return;
        }
        match n.state {
            State::Dirty if !n.has(F_QUEUED) => {
                let (h, id) = (n.height as usize, n.id);
                self.queue_push(h, id, x);
                self.queue_len += 1;
                self.set_flag(x, F_QUEUED, true);
            }
            State::Parked if !n.has(F_PARKSET) => {
                let key = (n.id, x);
                self.parked.insert(key);
                self.set_flag(x, F_PARKSET, true);
            }
            _ => {}
        }
    }

    fn dequeue(&mut self, x: Slot) {
        let n = self.n(x);
        if n.has(F_QUEUED) {
            self.queue_len -= 1;
            self.set_flag(x, F_QUEUED, false);
        }
        let n = self.n(x);
        if n.has(F_PARKSET) {
            let key = (n.id, x);
            self.parked.remove(&key);
            self.set_flag(x, F_PARKSET, false);
        }
    }

    fn set_height(&mut self, x: Slot, h: u32) {
        let n = self.n(x);
        if n.height == h {
            return;
        }
        let queued = n.has(F_QUEUED);
        let id = n.id;
        self.nm(x).height = h;
        if queued {
            // The old entry is dead (wrong height).
            self.queue_push(h as usize, id, x);
        }
        self.sync_order_lazy(x);
    }

    fn queue_push(&mut self, h: usize, id: NodeId, x: Slot) {
        if self.queue.len() <= h {
            self.queue.resize_with(h + 1, Vec::new);
        }
        self.queue[h].push((id, x));
        self.queue_min = self.queue_min.min(h);
    }

    fn queue_live(&self, h: usize, id: NodeId, x: Slot) -> bool {
        self.live(x) && {
            let n = self.n(x);
            n.id == id && n.has(F_QUEUED) && n.height as usize == h
        }
    }

    /// The lowest height with a live entry, its live entries sorted by id.
    fn queue_first(&mut self) -> Option<(u32, Vec<Slot>)> {
        if self.queue_len == 0 {
            self.queue.clear();
            self.queue_min = 0;
            return None;
        }
        while self.queue_min < self.queue.len() {
            let h = self.queue_min;
            let mut b = std::mem::take(&mut self.queue[h]);
            b.retain(|&(id, x)| self.queue_live(h, id, x));
            b.sort_unstable();
            b.dedup();
            if b.is_empty() {
                self.queue_min += 1;
                continue;
            }
            let out = b.iter().map(|e| e.1).collect();
            self.queue[h] = b;
            return Some((u32::try_from(h).expect("height"), out));
        }
        None
    }

    /// Defers the order summary update of a state or height change.
    fn sync_order_lazy(&mut self, s: Slot) {
        let n = self.n(s);
        if n.has(F_EFF_DYN) || n.has(F_CLAIM_DYN) || n.state == State::Inert {
            // Their bracket or reach summary depends on the state.
            self.sync_order(s);
        } else {
            self.order_dirty.push(s);
        }
    }

    /// Repairs the deferred order summaries, one pass per unit.
    fn flush_order(&mut self) {
        if self.order_dirty.is_empty() {
            return;
        }
        let mut d = std::mem::take(&mut self.order_dirty);
        d.retain(|&s| self.live(s) && !self.n(s).has(F_APP));
        let mut items: Vec<(u32, u64, OItem)> = d
            .iter()
            .map(|&s| (self.n(s).unit, self.n(s).label, self.oitem(s)))
            .collect();
        items.sort_unstable_by_key(|i| (i.0, i.1));
        items.dedup_by_key(|i| (i.0, i.1));
        let mut i = 0;
        while i < items.len() {
            let unit = items[i].0;
            let j = i + items[i..].iter().take_while(|x| x.0 == unit).count();
            let batch: Vec<(u64, OItem)> = items[i..j].iter().map(|x| (x.1, x.2)).collect();
            let root = self.units[unit as usize].root;
            self.order.set_many(&OAug, root, &batch);
            i = j;
        }
    }

    /// Raises `x` and the finished readers that must stay above it.
    fn raise(&mut self, x: Slot, h: u32, origin: Slot) {
        let mut stack = vec![(x, h)];
        let mut budget = 4 * self.ids.len() + 16;
        while let Some((y, h)) = stack.pop() {
            if y == origin || budget == 0 {
                continue;
            }
            budget -= 1;
            if !self.live(y) || self.n(y).height >= h {
                continue;
            }
            self.set_height(y, h);
            if let Some(a) = self.aux_of(y) {
                for &r in &a.out_readers {
                    if self.n(r).state == State::Done {
                        stack.push((r, h + 1));
                    }
                }
            }
            let unit = self.n(y).unit;
            for d in &self.n(y).defs {
                let from = y.pack();
                for (_, &r) in self.users.range((from, d.field, 0)..=(from, d.field, u64::MAX)) {
                    if self.n(r).state == State::Done {
                        stack.push((r, h + 1));
                    }
                }
            }
            let _ = unit;
        }
    }

    fn leave_spec(&mut self, x: Slot) {
        if self.n(x).state == State::Spec {
            self.spec_live -= 1;
        }
    }

    /// Marks `x` as needing a re-run.
    pub(crate) fn make_dirty(&mut self, x: Slot) {
        if self.tracing > 0 {
            self.note(x, Cause::Structure);
        }
        self.make_dirty_quiet(x);
    }

    /// `make_dirty` with the cause already noted.
    fn make_dirty_quiet(&mut self, x: Slot) {
        if !self.live(x) || self.n(x).state == State::Inert {
            return;
        }
        self.set_flag(x, F_REBIND, false);
        self.touch(x);
        let n = self.nm(x);
        n.epoch = n.epoch.wrapping_add(1);
        let st = n.state;
        match st {
            State::Dirty => return,
            State::Spec => {
                self.report.mispredictions += 1;
                self.leave_spec(x);
            }
            State::Parked => {
                self.dequeue(x);
            }
            State::Done if self.spec_done.remove(&x) => {
                self.report.mispredictions += 1;
            }
            _ => {}
        }
        count!(Dirty);
        self.nm(x).state = State::Dirty;
        self.nm(x).floor = 0;
        // New information: a stopped speculation may be tried again.
        self.set_flag(x, F_NOSPEC, false);
        self.sync_pending(x);
        self.sync_order_lazy(x);
        self.enqueue(x);
    }

    fn dirty_field_users(&mut self, site: u64, f: Field, cause: &Cause) {
        let users: Vec<Slot> = self
            .users
            .range((site, f, 0)..=(site, f, u64::MAX))
            .map(|(_, &u)| u)
            .collect();
        for u in users {
            if self.tracing > 0 {
                self.note(u, cause.clone());
            }
            self.make_dirty_quiet(u);
        }
    }

    fn dirty_loop_users(&mut self, f: Field) {
        let mut users: Vec<Slot> = self
            .loop_users
            .get(&f)
            .map_or_else(Default::default, |s| s.iter().copied().collect());
        users.sort_unstable();
        if self.tracing > 0 {
            for &u in &users {
                self.note(u, Cause::Loop { field: f });
            }
        }
        for u in users {
            self.make_dirty_quiet(u);
        }
    }

    fn park(&mut self, x: Slot, on: Slot) {
        count!(Park);
        self.dequeue(x);
        self.nm(x).state = State::Parked;
        self.nm(x).on = on;
        if on != NIL {
            let name = self.op(self.n(on).op).name();
            bump(&mut self.report.waits_by_op, name);
            self.ax(on).waiters.push(x);
            if self.n(on).has(F_INPEND) {
                self.report.pending_waits += 1;
            }
            let h = self.n(on).height + 1;
            let floor = self.n(x).floor.max(h);
            self.nm(x).floor = floor;
            self.raise(x, h, on);
        }
        self.sync_order_lazy(x);
        self.enqueue(x);
    }

    fn wake_waiters(&mut self, x: Slot) {
        let Some(a) = self.aux_of(x) else { return };
        if a.waiters.is_empty() {
            return;
        }
        let waiters = std::mem::take(&mut self.ax(x).waiters);
        let h = self.n(x).height + 1;
        for w in waiters {
            if self.live(w) && self.n(w).state == State::Parked && self.n(w).on == x {
                count!(Wake);
                if self.tracing > 0 {
                    self.note(w, Cause::Woken { by: self.n(x).id });
                }
                self.dequeue(w);
                self.nm(w).state = State::Dirty;
                let hw = self.n(w).height.max(h);
                self.set_height(w, hw);
                self.enqueue(w);
            }
        }
    }

    // --------------------------------------------------------- reachability

    /// What `x` keeps alive: its children, the definitions and outputs it
    /// read, and what it waits on.
    fn compute_deps(&self, x: Slot) -> Vec<Slot> {
        let n = self.n(x);
        let mut deps: Vec<Slot> = self.children_of(x).to_vec();
        for u in &n.uses {
            match u.kind {
                K_FIELD if u.site != NIL => deps.push(u.site),
                K_OUTPUT => deps.push(Slot::unpack(u.key)),
                _ => {}
            }
        }
        if n.on != NIL {
            // Kept until the node runs again (even once woken), so what it
            // waited on stays alive and demanded.
            deps.push(n.on);
        }
        deps.sort_unstable();
        deps.dedup();
        deps.retain(|&d| self.live(d));
        deps
    }

    /// Records the range a stream claimer's run read the structure of; its
    /// end label goes into the order summary (`OSum::maxs`), so a touch
    /// finds the claimers reaching it without a scan.
    fn set_stream(&mut self, x: Slot, range: Option<StreamRange>) {
        if self.aux_of(x).and_then(|a| a.stream) == range {
            return;
        }
        self.ax(x).stream = range;
        self.sync_order(x);
    }

    /// The structure at `label` changed (a node, definition or scope effect
    /// there came, went or changed): stream claimers that read it run again.
    /// A range ends just before the node after its delimiter, so that node
    /// going counts too.
    fn stream_touch(&mut self, unit: u32, label: u64) {
        if self.in_eval {
            self.touch_log.push((unit, label));
        }
        let hit: Vec<Slot> = self
            .stream_reaching(unit, label)
            .into_iter()
            .filter(|&x| {
                self.aux_of(x).and_then(|a| a.stream).is_some_and(|r| {
                    let (a, e) = self.stream_labels(x, r);
                    a < label && label <= e
                })
            })
            .collect();
        for x in hit {
            self.make_dirty(x);
        }
    }

    /// The stream claimers before `label` whose summarised range end
    /// reaches it (a superset of those whose range covers it: a claimer's
    /// range starts after its own unfolded text).
    fn stream_reaching(&self, unit: u32, label: u64) -> Vec<Slot> {
        let root = self.units[unit as usize].root;
        let mut before = label;
        let mut hit = Vec::new();
        // The unit's summary says at once when no range reaches this far
        // (the common case: a search would still descend to `label`).
        if self.order.sum(&OAug, root).maxs < label {
            return hit;
        }
        while let Some(k) = self.order.search_back(&OAug, root, before, &|s: OSum| s.maxs >= label) {
            if let Some(i) = self.order.get(root, k) {
                hit.push(i.slot);
            }
            before = k;
        }
        hit
    }

    /// The labels a recorded range spans now: `(a, e]`. A boundary node
    /// that is gone widens the range (the claimer re-runs anyway).
    fn stream_labels(&self, x: Slot, r: StreamRange) -> (u64, u64) {
        let a = if self.live(r.after) {
            self.n(r.after).label
        } else {
            self.n(x).label
        };
        let e = if r.end != NIL && self.live(r.end) {
            self.order_next(self.n(x).unit, self.n(r.end).label)
                .map_or(u64::MAX, |(k, _)| k)
        } else {
            u64::MAX
        };
        (a, e)
    }

    /// The label range a stream claim of `s` scans: after `s` and what it
    /// unfolded, up to the end of its parent's unfolded text (or the unit).
    pub(crate) fn stream_bounds(&self, s: Slot) -> (u32, Slot, Slot) {
        let n = self.n(s);
        let after = self.subtree_end(s);
        let end = if n.parent == NIL || !self.live(n.parent) {
            NIL
        } else {
            self.subtree_end(n.parent)
        };
        (n.unit, after, end)
    }

    /// The node before `label` in `unit`'s order.
    fn order_prev(&self, unit: u32, label: u64) -> Option<(u64, Slot)> {
        self.order
            .prev(self.units[unit as usize].root, label)
            .map(|(k, i)| (k, i.slot))
    }

    /// The next node after `label` in `unit`'s order.
    pub(crate) fn order_next(&self, unit: u32, label: u64) -> Option<(u64, Slot)> {
        self.order
            .next(self.units[unit as usize].root, label)
            .map(|(k, i)| (k, i.slot))
    }

    /// `x` goes away (or out of the program) before finishing: speculation
    /// that rested on it will never be confirmed, so it runs again.
    fn refute_assumers(&mut self, x: Slot) {
        let assumers = match self.aux_of(x) {
            Some(a) if !a.assumers.is_empty() => a.assumers.clone(),
            _ => return,
        };
        self.ax(x).assumers.clear();
        for a in assumers {
            if self.live(a.node) && self.n(a.node).epoch == a.epoch && self.n(a.node).state == State::Spec {
                self.make_dirty(a.node);
            }
        }
    }

    // -------------------------------------------------------------- freeing

    #[allow(clippy::too_many_lines)]
    /// Frees a run of sibling subtrees. A balanced run (its brackets close
    /// what they open) changes no binding outside itself, so its effects
    /// are cleared first and the nodes go without region re-binding: the
    /// cost is the run's size, not the unit's. (Freed one at a time, a
    /// close removed before its open would re-bind up to the unit's end.)
    /// `quiet` skips the check: the whole unit is going.
    fn free_span(&mut self, kids: &[Slot], quiet: bool) {
        let live: Vec<Slot> = kids.iter().copied().filter(|&k| self.live(k)).collect();
        let Some((&first, &last)) = live.first().zip(live.last()) else {
            return;
        };
        let balanced = quiet || {
            let unit = self.n(first).unit;
            let root = self.units[unit as usize].root;
            let lo = self.n(first).label;
            let hi = self.n(self.subtree_end(last)).label;
            let span = self.order.fold(&OAug, root, lo, hi + 1);
            span.c == 0 && span.o == 0
        };
        if balanced {
            for &k in &live {
                if self.oitem(k).eff != EFF_NONE {
                    self.nm(k).effect = EFF_NONE;
                    self.nm(k).bound = EFF_NONE;
                    self.sync_order(k);
                }
            }
        }
        for &k in live.iter().rev() {
            if self.live(k) {
                self.free_node(k);
            }
        }
    }

    /// The readers of `x`'s output forget it and run again.
    fn detach_readers(&mut self, x: Slot, readers: &[Slot]) {
        let key = x.pack();
        for &r in readers {
            if !self.live(r) || r == x {
                continue;
            }
            let n = self.nm(r);
            if n.uses.iter().any(|u| u.kind == K_OUTPUT && u.key == key) {
                let uses: Vec<Use> = n
                    .uses
                    .iter()
                    .copied()
                    .filter(|u| !(u.kind == K_OUTPUT && u.key == key))
                    .collect();
                n.uses = uses.into_boxed_slice();
            }
            self.make_dirty(r);
        }
    }

    fn free_node(&mut self, x: Slot) {
        count!(Free);
        self.touch(x);
        let kids = self.aux_of(x).map_or_else(Default::default, |a| a.children.clone());
        if !kids.is_empty() {
            self.ax(x).children.clear();
            let dying_unit = self.n(x).has(F_APP);
            self.free_span(&kids, dying_unit);
        }
        let claimed = self.aux_of(x).map_or_else(Default::default, |a| a.claimed.clone());
        if !claimed.is_empty() {
            self.ax(x).claimed.clear();
            for s in claimed {
                if self.live(s) && self.aux_of(s).and_then(|a| a.claimed_by) == Some(x) {
                    self.activate(s);
                }
            }
        }
        if let Some(by) = self.aux_of(x).and_then(|a| a.claimed_by)
            && self.live(by)
        {
            let mut l = self.aux_of(by).map_or_else(Default::default, |a| a.claimed.clone());
            l.retain(|&s| s != x);
            self.ax(by).claimed = l;
            // The claimed text is one of the claimer's inputs: it runs
            // again (and its claim is recomputed) whatever freed `x`.
            self.claim_dirty.insert(by);
            self.make_dirty(by);
        }
        let defs: Vec<Field> = self.n(x).defs.iter().map(|d| d.field).collect();
        for f in defs {
            self.remove_def(x, f);
        }
        self.clear_contribs(x);
        self.drop_watch(x);
        self.leave_spec(x);
        self.dequeue(x);
        let old_eff = self.oitem(x).eff;
        if self.n(x).has(F_INPEND) {
            self.pending_index(x, false);
            self.set_flag(x, F_INPEND, false);
        }
        self.set_uses(x, Vec::new());
        let (readers, waiters) = self
            .aux_of(x)
            .map_or_else(Default::default, |a| (a.out_readers.clone(), a.waiters.clone()));
        // Readers and waiters drop their references to `x` (its slot may be
        // reused), keeping demand counts balanced.
        self.detach_readers(x, &readers);
        self.release_waiters(x, &waiters);
        self.refute_assumers(x);
        self.set_stream(x, None);
        let (u, l) = (self.n(x).unit, self.n(x).label);
        self.stream_touch(u, l);
        self.set_publishes(x, Vec::new());
        self.roots.remove(&x);
        self.claim_dirty.remove(&x);
        let n = self.n(x);
        let (unit, label, is_app) = (n.unit, n.label, n.has(F_APP));
        if is_app {
            let key = (n.op, n.args);
            self.apps_live -= 1;
            self.memo.remove(&key);
            let body = unit;
            self.units[body as usize].live = false;
            self.units[body as usize].root = TNIL;
            self.units[body as usize].env.clear();
            self.units[body as usize].accs.clear();
            self.units[body as usize].watches.clear();
            self.free_units.push(body);
        } else {
            self.nm(x).state = State::Done;
            let root = self.units[unit as usize].root;
            let (r, _) = self.order.remove(&OAug, root, label);
            self.units[unit as usize].root = r;
            if old_eff != EFF_NONE {
                // The bracket disappeared: re-bind the region it shaped.
                let after = self.order.next(r, label).map(|(_, i)| i.slot);
                if let Some(a) = after {
                    let o = self.oitem(a).eff;
                    self.effect_region_changed_at(unit, label, old_eff, o, a);
                }
            }
        }
        let id = self.n(x).id;
        self.ids.remove(&id);
        let a = self.n(x).aux;
        if a != NO_AUX {
            self.aux[a as usize] = Aux::default();
            self.free_aux.push(a);
        }
        let n = self.nm(x);
        n.state = State::Free;
        n.gen_ = n.gen_.wrapping_add(1);
        n.aux = NO_AUX;
        n.uses = Box::new([]);
        n.defs = Box::new([]);
        n.flags = 0;
        self.free_slots.push(x.ix);
    }

    /// After removing a bracket node at `label`: re-bind the region it
    /// shaped, starting from the node after it.
    fn effect_region_changed_at(&mut self, unit: u32, label: u64, old: u8, _next_eff: u8, next: Slot) {
        span!(Region);
        let root = self.units[unit as usize].root;
        let before = self.order.fold(&OAug, root, 0, label);
        let d = OAug
            .combine(
                before,
                OAug.leaf(&OItem {
                    slot: NIL,
                    eff: old,
                    pend: false,
                    work: false,
                    h: 0,
                    reach: 0,
                    hit: 0,
                    sreach: 0,
                }),
            )
            .o
            .max(before.o);
        let end = if d == 0 {
            label
        } else {
            self.order
                .search_fwd(&OAug, root, label, &|s: OSum| s.c >= d)
                .unwrap_or(u64::MAX)
        };
        let mut region = Vec::new();
        self.order.range(root, label, end, &mut region, usize::MAX);
        let _ = next;
        self.report.scope_region += region.len() as u64;
        count!(RegionNode, region.len());
        for (_, s) in region {
            if !self.live(s) || self.n(s).state == State::Inert {
                continue;
            }
            self.rescope_node(unit, s);
        }
    }

    // ------------------------------------------------------- accumulators

    fn monoid_aug(&self, f: Field) -> Option<(&dyn Monoid, ValueId)> {
        self.monoids.get(&f).map(|m| (&**m, m.empty(&self.values)))
    }

    fn acc_set(&mut self, unit: u32, f: Field, label: u64, v: Option<ValueId>) {
        if !self.monoids.contains_key(&f) {
            return;
        }
        let root = self.units[unit as usize].accs.get(&f).copied().unwrap_or(TNIL);
        let r = match v {
            // In place when the key is there: one path of cheap summaries.
            Some(v) if self.accs_arena.update(&AccAug, root, label, &mut |x| *x = v) => root,
            Some(v) => self.accs_arena.insert(&AccAug, root, label, v),
            None => self.accs_arena.remove(&AccAug, root, label).0,
        };
        self.units[unit as usize].accs.insert(f, r);
    }

    pub(crate) fn acc_fold(&self, unit: u32, f: Field, lo: u64, hi: u64) -> ValueId {
        let Some((m, empty)) = self.monoid_aug(f) else {
            return ValueId::UNIT;
        };
        let b = AccBuild {
            m,
            v: &self.values,
            empty,
            cache: &self.acc_cache,
        };
        let root = self.units[unit as usize].accs.get(&f).copied().unwrap_or(TNIL);
        self.accs_arena.lazy_fold(&b, root, lo, hi)
    }

    fn node_contrib_folds(&self, x: Slot) -> Vec<(Field, ValueId)> {
        let Some(a) = self.aux_of(x) else { return Vec::new() };
        let mut out: Vec<(Field, ValueId)> = Vec::new();
        for &(f, v) in &a.contribs {
            if let Some(e) = out.iter_mut().find(|e| e.0 == f) {
                if let Some(m) = self.monoids.get(&f) {
                    e.1 = m.combine(&self.values, e.1, v);
                }
            } else {
                out.push((f, v));
            }
        }
        out
    }

    fn set_contribs(&mut self, x: Slot, new: Vec<(Field, ValueId)>) {
        let old = self.aux_of(x).map_or_else(Default::default, |a| a.contribs.clone());
        if old == new {
            return;
        }
        let old_folds = self.node_contrib_folds(x);
        self.ax(x).contribs = new;
        let new_folds = self.node_contrib_folds(x);
        let (unit, label) = (self.n(x).unit, self.n(x).label);
        let mut fields: Vec<Field> = old_folds.iter().chain(&new_folds).map(|e| e.0).collect();
        fields.sort_unstable();
        fields.dedup();
        for f in fields {
            let o = old_folds.iter().find(|e| e.0 == f).map(|e| e.1);
            let n = new_folds.iter().find(|e| e.0 == f).map(|e| e.1);
            if o != n {
                self.acc_set(unit, f, label, n);
                self.stab_contrib(unit, label, f);
            }
        }
    }

    fn clear_contribs(&mut self, x: Slot) {
        if self.aux_of(x).is_some_and(|a| !a.contribs.is_empty()) {
            self.set_contribs(x, Vec::new());
        }
    }

    fn watch_key(&self, reader: Slot, anchor: Slot) -> (u64, Slot) {
        let from = if anchor == NIL || !self.live(anchor) {
            0
        } else {
            self.n(anchor).label + 1
        };
        (from, reader)
    }

    fn set_watch(&mut self, reader: Slot, w: Option<(Field, Slot, ValueId, bool)>) {
        self.drop_watch(reader);
        let Some((field, anchor, seen, waiting)) = w else {
            return;
        };
        let unit = self.n(reader).unit;
        let key = self.watch_key(reader, anchor);
        let span = self.n(reader).label.saturating_sub(key.0);
        let u = &mut self.units[unit as usize];
        u.max_span = u.max_span.max(span);
        u.watches.insert(
            key,
            Watch {
                field,
                anchor,
                seen,
                waiting,
            },
        );
        self.ax(reader).watch = Some((field, anchor));
    }

    fn drop_watch(&mut self, reader: Slot) {
        if let Some((_, anchor)) = self.aux_of(reader).and_then(|a| a.watch) {
            let key = self.watch_key(reader, anchor);
            let unit = self.n(reader).unit;
            self.units[unit as usize].watches.remove(&key);
            self.ax(reader).watch = None;
        }
    }

    fn rebuild_watches(&mut self, unit: u32) {
        let old = std::mem::take(&mut self.units[unit as usize].watches);
        self.units[unit as usize].max_span = 0;
        for ((_, reader), w) in old {
            if self.live(reader) {
                self.ax(reader).watch = None;
                self.set_watch(reader, Some((w.field, w.anchor, w.seen, w.waiting)));
            }
        }
    }

    /// Watches whose range `[from, reader)` contains `label`.
    fn stab(&self, unit: u32, label: u64) -> Vec<(Slot, Watch)> {
        let u = &self.units[unit as usize];
        let lo = label.saturating_sub(u.max_span);
        u.watches
            .range((lo, Slot { ix: 0, gen_: 0 })..=(label, NIL))
            .filter(|((from, r), _)| *from <= label && label < self.n(*r).label)
            .map(|((_, r), w)| (*r, *w))
            .collect()
    }

    fn stab_contrib(&mut self, unit: u32, label: u64, f: Field) {
        for (r, w) in self.stab(unit, label) {
            if w.field != f || w.waiting {
                continue;
            }
            let lo = self.watch_key(r, w.anchor).0;
            let now = self.acc_fold(unit, f, lo, self.n(r).label);
            if now != w.seen {
                self.make_dirty(r);
            }
        }
    }

    fn stab_structure(&mut self, unit: u32, label: u64) {
        for (r, _) in self.stab(unit, label) {
            self.make_dirty(r);
        }
    }

    /// Readers whose range starts right after `s` run again: they chose
    /// `s` as their anchor (a previous sibling), and it is gone as one.
    fn unanchor_watches(&mut self, s: Slot) {
        let unit = self.n(s).unit;
        let from = self.n(s).label + 1;
        let readers: Vec<Slot> = self.units[unit as usize]
            .watches
            .range((from, Slot { ix: 0, gen_: 0 })..=(from, NIL))
            .map(|((_, r), _)| *r)
            .collect();
        for r in readers {
            self.make_dirty(r);
        }
    }

    /// Checks the range readers over every position finished this batch.
    fn settle_deferred(&mut self) {
        if self.settle_pending.is_empty() {
            return;
        }
        let mut d = std::mem::take(&mut self.settle_pending);
        d.sort_unstable();
        d.dedup();
        self.flush_order();
        for (u, l) in d {
            self.stab_settle(u, l);
        }
    }

    fn stab_settle(&mut self, unit: u32, label: u64) {
        self.flush_order();
        for (r, w) in self.stab(unit, label) {
            if !w.waiting || self.n(r).state != State::Parked {
                continue;
            }
            let lo = self.watch_key(r, w.anchor).0;
            let s = self.ord_fold(unit, lo, self.n(r).label);
            if s.work == 0 {
                self.drop_watch(r);
                self.dequeue(r);
                self.nm(r).state = State::Dirty;
                let h = self.n(r).height.max(s.maxh + 1);
                self.set_height(r, h);
                self.enqueue(r);
            }
        }
    }

    // ----------------------------------------------------- applying results

    fn set_uses(&mut self, x: Slot, new: Vec<Use>) {
        span!(Uses);
        let n = self.n(x);
        let (unit, label) = (n.unit, n.label);
        let has_label = !n.has(F_APP);
        let old = std::mem::replace(&mut self.nm(x).uses, new.into_boxed_slice());
        for u in &old {
            match u.kind {
                K_FIELD | K_CHAIN | K_ENV if has_label => {
                    self.users
                        .remove(&(site_key(unit, u.site), Field(ValueId(u.key)), label));
                }
                K_OUTPUT => {
                    let t = Slot::unpack(u.key);
                    if self.live(t) && self.n(t).aux != NO_AUX {
                        self.ax(t).out_readers.retain(|&r| r != x);
                    }
                }
                K_LOOP => {
                    if let Some(s) = self.loop_users.get_mut(&Field(ValueId(u.key))) {
                        s.remove(&x);
                    }
                }
                _ => {}
            }
        }
        let new: Vec<Use> = self.n(x).uses.to_vec();
        for u in new {
            match u.kind {
                K_FIELD | K_CHAIN | K_ENV if has_label => {
                    self.users
                        .insert((site_key(unit, u.site), Field(ValueId(u.key)), label), x);
                }
                K_OUTPUT => {
                    let t = Slot::unpack(u.key);
                    if self.live(t) {
                        let a = self.ax(t);
                        if !a.out_readers.contains(&x) {
                            a.out_readers.push(x);
                        }
                    }
                }
                K_LOOP => {
                    self.loop_users.entry(Field(ValueId(u.key))).or_default().insert(x);
                }
                _ => {}
            }
        }
    }

    fn set_publishes(&mut self, x: Slot, pubs: Vec<(Field, ValueId)>) {
        let id = self.n(x).id;
        let old = self.aux_of(x).map_or_else(Default::default, |a| a.publishes.clone());
        if old.is_empty() && pubs.is_empty() {
            return;
        }
        for (f, _) in old {
            if let Some(m) = self.loop_pubs.get_mut(&f) {
                m.remove(&id);
            }
        }
        for &(f, v) in &pubs {
            self.loop_pubs.entry(f).or_default().insert(id, v);
        }
        self.ax(x).publishes = pubs;
    }

    /// Canonical children ids: parent id, text, and occurrence among equal
    /// siblings.
    fn child_ids(&self, x: Slot, specs: &[(OpId, ValueId)]) -> Vec<NodeId> {
        let pid = self.n(x).id.0;
        let mut seen: FxHashMap<(OpId, ValueId), u64> = FxHashMap::default();
        specs
            .iter()
            .map(|&(op, args)| {
                let k = seen.entry((op, args)).or_default();
                let id = NodeId(hash_words(13, &[pid, u64::from(op.0), args.0, *k]));
                *k += 1;
                id
            })
            .collect()
    }

    #[allow(clippy::too_many_lines)]
    fn set_children(&mut self, x: Slot, specs: &[(OpId, ValueId)], follows: usize, heights: &[u32], provisional: bool) {
        let old = self.aux_of(x).map_or_else(Default::default, |a| a.children.clone());
        if old.is_empty() && specs.is_empty() {
            return;
        }
        // Continuations are the last `follows` specs: siblings of `x` in the
        // tree (its own depth), owned by its run like children.
        let follows_at = specs.len().saturating_sub(follows);
        let ids = self.child_ids(x, specs);
        let old_ids: Vec<NodeId> = old.iter().map(|&c| self.n(c).id).collect();
        let body = self.aux_of(x).and_then(|a| a.body);
        let unit = body.unwrap_or(self.n(x).unit);
        // The children follow the claimed text (a body is a unit of its
        // own). A claim that changed moved that anchor: children already
        // there are made again at the new place.
        let anchor = if body.is_some() {
            None
        } else {
            let last_claimed = self.aux_of(x).and_then(|a| a.claimed.last().copied());
            Some(last_claimed.map_or(x, |c| self.subtree_end(c)))
        };
        let placed = match (anchor, old.first()) {
            (Some(a), Some(&c)) => self.order_prev(unit, self.n(c).label).is_some_and(|(_, p)| p == a),
            _ => true,
        } && old
            .iter()
            .enumerate()
            .all(|(i, &c)| (self.n(c).parent != x) == (i >= follows_at));
        if old_ids == ids && placed {
            if !provisional {
                for &c in &old {
                    self.clear_prov(c);
                }
            }
            return;
        }
        let mut pre = 0;
        while placed && pre < old.len() && pre < ids.len() && old_ids[pre] == ids[pre] {
            pre += 1;
        }
        let mut suf = 0;
        while placed
            && suf < old.len() - pre
            && suf < ids.len() - pre
            && old_ids[old.len() - 1 - suf] == ids[ids.len() - 1 - suf]
        {
            suf += 1;
        }
        self.free_span(&old[pre..old.len() - suf], false);
        let after = if pre == 0 {
            anchor
        } else {
            Some(self.subtree_end(old[pre - 1]))
        };
        // Labels for the whole run of new children at once: one by one,
        // each would halve the gap and soon force a renumbering.
        let labels = if ids.len() > pre + suf {
            self.alloc_labels(unit, after, ids.len() - pre - suf)
        } else {
            Vec::new()
        };
        let mut kids: Vec<Slot> = old[..pre].to_vec();
        let parent = if body.is_some() { NIL } else { x };
        // The new text is inserted as a whole: first the records, then the
        // scope effects (a balanced span changes no binding outside itself,
        // so needs no re-binding), then the definitions.
        let mut created = Vec::new();
        let depth = {
            let mut d = 0u32;
            let mut p = x;
            while p != NIL && self.live(p) {
                d += 1;
                p = self.n(p).parent;
            }
            d
        };
        if ids.len() > pre + suf {
            let n_new = u32::try_from(ids.len() - pre - suf).unwrap_or(u32::MAX);
            let e = self.created_in_eval.entry(x).or_default();
            e.0 = e.0.saturating_add(n_new);
            for &spec in &specs[pre..ids.len() - suf] {
                if e.1.len() == 4 {
                    e.1.remove(0);
                }
                e.1.push(spec);
            }
            let (created_n, last) = (e.0, e.1.clone());
            if created_n > self.limits.children_per_node || depth > self.limits.unfold_depth {
                let last = last
                    .iter()
                    .map(|&(op, args)| {
                        (
                            self.ops[op.0 as usize].name().to_string(),
                            self.values.render(args).chars().take(80).collect(),
                        )
                    })
                    .collect();
                self.errors.push(EvalError::RunawayCreation {
                    node: self.n(x).id,
                    created: created_n,
                    depth,
                    last,
                });
            }
        }
        for i in pre..ids.len() - suf {
            let (op, args) = specs[i];
            let follow = i >= follows_at;
            let c = self.create_raw(
                unit,
                after,
                Some(labels[i - pre]),
                op,
                args,
                if follow { self.n(x).parent } else { parent },
                ids[i],
                provisional,
            );
            if body.is_some() && !follow {
                // Body nodes belong to the application.
                self.nm(c).parent = x;
            }
            created.push(c);
        }
        if let (Some(&first), Some(&last)) = (created.first(), created.last()) {
            for &c in &created {
                self.set_created_effect(c, false);
            }
            let root = self.units[unit as usize].root;
            let span = self
                .order
                .fold(&OAug, root, self.n(first).label, self.n(last).label + 1);
            let balanced = span.c == 0 && span.o == 0;
            if !balanced {
                // Unbalanced: re-bind as if added one at a time.
                let effs: Vec<u8> = created.iter().map(|&c| self.n(c).effect).collect();
                for &c in &created {
                    self.nm(c).effect = EFF_NONE;
                    self.sync_order(c);
                }
                for (&c, &e) in created.iter().zip(&effs) {
                    if e != EFF_NONE {
                        self.nm(c).effect = e;
                        self.sync_order(c);
                        self.effect_region_changed(c, EFF_NONE);
                    }
                }
            }
            // The run's scope depths: one fold at its start, then a prefix
            // count of the brackets inside it.
            let mut depth = self.order.fold(&OAug, root, 0, self.n(first).label).o;
            let mut rd = std::mem::take(&mut self.run_depths);
            rd.clear();
            for &c in &created {
                let eff = self.oitem(c).eff;
                rd.push((self.n(c).label, eff, depth));
                depth = eff_opens(eff) + depth.saturating_sub(eff_closes(eff));
            }
            self.run_depths = rd;
            // Balanced, every opener's scope lies in the new text, where
            // nothing has run: its markers have no readers to take.
            self.fresh_scope = balanced;
            for &c in &created {
                self.set_created_kind(c);
            }
            self.fresh_scope = false;
            self.bind_fresh(&created, &heights[pre.min(heights.len())..]);
            self.run_depths.clear();
        }
        kids.extend_from_slice(&created);
        kids.extend_from_slice(&old[old.len() - suf..]);
        self.ax(x).children.clone_from(&kids);
        if !provisional {
            for c in kids {
                self.clear_prov(c);
            }
        }
    }

    fn clear_prov(&mut self, c: Slot) {
        let mut stack = vec![c];
        while let Some(s) = stack.pop() {
            if !self.live(s) || !self.n(s).has(F_PROV) {
                continue;
            }
            self.set_flag(s, F_PROV, false);
            self.enqueue(s);
            stack.extend(self.children_of(s).iter().copied());
        }
    }

    /// Applies a run's definitions; returns whether any value changed.
    fn set_defs(&mut self, x: Slot, defs: &[(Field, ValueId, bool)]) -> bool {
        if self.n(x).has(F_DYN) {
            let set: Vec<(Field, bool)> = defs.iter().map(|d| (d.0, d.2)).collect();
            self.set_defset(x, &set);
        } else {
            let declared: Vec<Field> = self
                .n(x)
                .defs
                .iter()
                .filter(|d| self.is_client_def(x, d.field) && d.value != self.seq_start)
                .map(|d| d.field)
                .collect();
            for &(f, _, _) in defs {
                if !declared.contains(&f) && self.is_client_def(x, f) {
                    let id = self.n(x).id;
                    let field = self.client_name(f);
                    self.errors.push(EvalError::UndeclaredDefinition { node: id, field });
                }
            }
            for &f in &declared {
                if !defs.iter().any(|d| d.0 == f) {
                    let id = self.n(x).id;
                    let field = self.client_name(f);
                    self.errors.push(EvalError::UndeclaredDefinition { node: id, field });
                }
            }
        }
        let mut changed = false;
        for &(f, v, _) in defs {
            if self.is_seq_hidden(f) && eff_opens(self.oitem(x).eff) > 0 {
                continue;
            }
            let old = self.n(x).def_value(f);
            if old != Some(v) {
                changed = true;
                let n = self.nm(x);
                for d in &mut n.defs {
                    if d.field == f {
                        d.value = v;
                    }
                }
                let cause = Cause::Changed {
                    field: f,
                    site: Some(self.n(x).id),
                };
                self.dirty_field_users(x.pack(), f, &cause);
            }
        }
        changed
    }

    /// A node dirtied only by re-binding: if every recorded read resolves to
    /// the same settled site and value as in its last run, the run would be
    /// identical, so the node is settled without running.
    fn revalidate(&mut self, x: Slot) -> bool {
        let ok = self.revalidate_inner(x);
        if ok {
            count!(RevalidateOk);
        } else {
            count!(RevalidateMiss);
        }
        ok
    }

    fn revalidate_inner(&mut self, x: Slot) -> bool {
        self.set_flag(x, F_REBIND, false);
        let n = self.n(x);
        if n.has(F_CLAIMER) || n.has(F_APP) || self.aux_of(x).is_some_and(|a| a.watch.is_some()) {
            return false;
        }
        let mut sig = 0u64;
        for u in &n.uses {
            let (sid, v) = match u.kind {
                K_FIELD => {
                    let f = Field(ValueId(u.key));
                    let w = self.walk(n.unit, n.label, f);
                    if w.pending != NIL || w.site != u.site {
                        return false;
                    }
                    if w.site == NIL {
                        (0, self.start_value(n.unit, f))
                    } else {
                        if !self.settled(w.site) {
                            return false;
                        }
                        (self.n(w.site).id.raw(), self.n(w.site).def_value(f))
                    }
                }
                K_LOOP => (0, self.loop_vals.get(&Field(ValueId(u.key))).copied()),
                K_ENV => match self.env_value(self.n(x).unit, Field(ValueId(u.key))) {
                    Ok(v) => (0, v),
                    Err(()) => return false,
                },
                K_OUTPUT => {
                    let t = Slot::unpack(u.key);
                    if !self.live(t) || !self.settled(t) {
                        return false;
                    }
                    let tn = self.n(t);
                    // The output read is keyed by the target's id.
                    sig = sig.wrapping_add(hash_words(
                        20,
                        &[
                            u64::from(K_OUTPUT),
                            tn.id.raw(),
                            0,
                            tn.has_ran().then_some(tn.output).map_or(0, ValueId::raw),
                        ],
                    ));
                    continue;
                }
                _ => return false,
            };
            sig = sig.wrapping_add(hash_words(
                20,
                &[u64::from(u.kind), u.key, sid, v.map_or(0, ValueId::raw)],
            ));
        }
        if sig != n.sig {
            return false;
        }
        let h = self.target_height_mut(x);
        self.finish(x, h);
        true
    }

    /// [`Self::target_height`] with deferred order summaries repaired first
    /// when it reads them (range readers).
    fn target_height_mut(&mut self, x: Slot) -> u32 {
        if self.aux_of(x).is_some_and(|a| a.watch.is_some()) {
            self.flush_order();
        }
        self.target_height(x)
    }

    fn target_height(&self, x: Slot) -> u32 {
        let n = self.n(x);
        let mut h = n.floor;
        for u in &n.uses {
            let t = match u.kind {
                K_FIELD if u.site != NIL => u.site,
                K_OUTPUT => Slot::unpack(u.key),
                _ => continue,
            };
            if self.live(t) {
                h = h.max(self.n(t).height + 1);
            }
        }
        if let Some((_, anchor)) = self.aux_of(x).and_then(|a| a.watch) {
            let lo = self.watch_key(x, anchor).0;
            let s = self.ord_fold(n.unit, lo, n.label);
            h = h.max(s.maxh + 1);
        }
        h
    }

    fn compute_taint(&self, x: Slot) -> bool {
        let n = self.n(x);
        if n.parent != NIL && self.live(n.parent) && self.n(n.parent).has(F_TAINT) {
            return true;
        }
        n.uses.iter().any(|u| match u.kind {
            K_LOOP => true,
            K_FIELD if u.site != NIL => self.n(u.site).has(F_TAINT),
            K_OUTPUT => {
                let t = Slot::unpack(u.key);
                self.live(t) && self.n(t).has(F_TAINT)
            }
            _ => false,
        })
    }

    fn update_taint(&mut self, x: Slot) {
        let mut stack = vec![x];
        while let Some(y) = stack.pop() {
            if !self.live(y) {
                continue;
            }
            let t = self.compute_taint(y);
            if t == self.n(y).has(F_TAINT) {
                continue;
            }
            self.set_flag(y, F_TAINT, t);
            if let Some(a) = self.aux_of(y) {
                stack.extend(a.out_readers.iter().copied());
                stack.extend(a.children.iter().copied());
            }
            for d in &self.n(y).defs {
                let from = y.pack();
                stack.extend(
                    self.users
                        .range((from, d.field, 0)..=(from, d.field, u64::MAX))
                        .map(|(_, &u)| u),
                );
            }
        }
    }

    fn touch(&mut self, x: Slot) {
        let n = self.n(x);
        if self.touched.contains_key(&n.id) {
            return;
        }
        let snap = Snap {
            existed: true,
            sig: n.sig,
            args: n.args,
            out: self.out_hash(x),
            sites: self.site_list(x),
        };
        let id = n.id;
        self.touched.insert(id, snap);
    }

    fn out_hash(&self, x: Slot) -> u64 {
        let n = self.n(x);
        let mut ws: Vec<u64> = vec![n.output.0];
        let mut ds: Vec<(u64, u64)> = n.defs.iter().map(|d| (d.field.0.0, d.value.0)).collect();
        ds.sort_unstable();
        for (f, v) in ds {
            ws.push(f);
            ws.push(v);
        }
        for &c in self.children_of(x) {
            ws.push(self.n(c).id.0);
        }
        hash_words(14, &ws)
    }

    fn site_list(&self, x: Slot) -> Vec<(u64, u64)> {
        let n = self.n(x);
        let mut v: Vec<(u64, u64)> = n
            .uses
            .iter()
            .filter(|u| u.kind == K_FIELD)
            .map(|u| (u.key, if u.site == NIL { 0 } else { self.n(u.site).id.0 }))
            .collect();
        v.sort_unstable();
        v
    }

    #[allow(clippy::too_many_lines)]
    fn apply(&mut self, r: RunResult) {
        let x = r.slot;
        if !self.live(x) || self.n(x).id != r.node || self.n(x).epoch != r.epoch {
            return;
        }
        let st = self.n(x).state;
        if !matches!(st, State::Dirty | State::Parked) || (st == State::Parked && !r.speculative) {
            return;
        }
        // A definition the run read was removed while it ran (its node
        // freed or re-run by an unfolding parent applied in the same
        // batch): only committed uses were re-bound, so run again.
        if r.uses.iter().any(|u| {
            (u.kind == K_FIELD || u.kind == K_CHAIN)
                && u.site != NIL
                && (!self.live(u.site) || self.n(u.site).def_value(Field(ValueId(u.key))).is_none())
        }) {
            count!(StaleSite);
            if r.speculative {
                // A guess that read a definition not produced yet (or gone):
                // dropped, and the node keeps its state. Dirtying a parked
                // node here would undo its wait every round.
                self.set_flag(x, F_NOSPEC, true);
            }
            return;
        }
        // The structure a stream claim scanned changed while it ran (or
        // the labels it compares against were renumbered meanwhile).
        if let Some(sr) = r.stream {
            let unit = self.n(x).unit;
            let (a, e) = self.stream_labels(x, sr);
            if r.relabels != self.relabels
                || self.touch_log[r.touch_seq.min(self.touch_log.len())..]
                    .iter()
                    .any(|&(u, l)| u == unit && a < l && l <= e)
            {
                count!(StaleSite);
                if r.speculative {
                    self.set_flag(x, F_NOSPEC, true);
                }
                return;
            }
        }
        self.report.text_memo_hits += u64::from(r.memo[0]);
        self.report.text_memo_misses += u64::from(r.memo[1]);
        self.report.app_memo_hits += u64::from(r.memo[2]);
        self.report.app_memo_misses += u64::from(r.memo[3]);
        if r.speculative {
            match r.end {
                End::Output(_) => {}
                End::OutOfFuel => {
                    self.report.fuel_outs += 1;
                    self.report.mispredictions += 1;
                    self.set_flag(x, F_NOSPEC, true);
                    return;
                }
                _ => {
                    // Stopped (waiting on something unresolved): the same
                    // guess would stop again every round until then; it
                    // is tried again once something it depends on changes.
                    self.set_flag(x, F_NOSPEC, true);
                    return;
                }
            }
            if r.error.is_some() {
                return;
            }
            if !self.stale_assumptions_hold(x, &r) {
                // A target finished earlier in this batch, against the
                // prediction: the run is discarded and the node runs again.
                self.report.mispredictions += 1;
                return;
            }
        }
        if let Some(e) = r.error.clone() {
            self.errors.push(e);
        }
        let output = match r.end {
            End::Output(v) => v,
            End::Unpredictable | End::OutOfFuel => return,
            End::Gated(t) => {
                self.report.gated += 1;
                count!(Gated);
                if self.live(t) && self.n(t).in_work() {
                    self.park(x, t);
                } else if self.live(t) {
                    let h = (self.n(t).height + 1).max(self.n(x).height);
                    self.set_height(x, h);
                }
                return;
            }
            End::GateRaise(h) => {
                let h = h.max(self.n(x).height);
                self.set_height(x, h);
                return;
            }
            End::Raise(h) => {
                self.report.stops += 1;
                let h = h.max(self.n(x).height + 1);
                self.set_height(x, h);
                return;
            }
            End::Wait(t) => {
                self.report.stops += 1;
                self.apply_partial(x, &r);
                if self.live(t) && self.n(t).in_work() {
                    self.park(x, t);
                } else if self.live(t) {
                    let h = (self.n(t).height + 1).max(self.n(x).height);
                    self.set_height(x, h);
                }
                return;
            }
            End::WaitEnv(f) => {
                // Until the client supplies the field (or declares it
                // absent): the node stays in work, so what it may define
                // keeps its readers waiting too.
                self.report.stops += 1;
                self.apply_partial(x, &r);
                self.park(x, NIL);
                self.env_waiters.entry(f).or_default().push(x);
                return;
            }
            End::NeedApp(op, args) => {
                self.report.stops += 1;
                let t = match self.memo.get(&(op, args)) {
                    Some(&t) => t,
                    None => self.create_app(op, args),
                };
                self.apply_partial(x, &r);
                self.park(x, t);
                return;
            }
            End::NeedChild(i) => {
                self.report.stops += 1;
                self.apply_partial(x, &r);
                if let Some(&c) = self.children_of(x).get(i)
                    && self.n(c).in_work()
                {
                    self.park(x, c);
                }
                return;
            }
            End::WaitRange(field, anchor) => {
                self.report.stops += 1;
                self.apply_partial(x, &r);
                self.park(x, NIL);
                self.set_watch(x, Some((field, anchor, ValueId::NONE, true)));
                // It may already be settled.
                let (u, l) = (self.n(x).unit, self.n(x).label);
                let lo = self.watch_key(x, anchor).0;
                self.flush_order();
                if self.ord_fold(u, lo, l).work == 0 {
                    self.stab_settle(u, lo);
                }
                return;
            }
        };
        self.touch(x);
        let provisional = r.speculative && !r.assumptions.is_empty();
        if r.speculative && self.shape_changes(x, &r) {
            // A guess that changes the structure (claim, unfolding, scope
            // effect or definition set) is not applied: it would re-bind
            // and dirty its surroundings on a value that may still change,
            // and the next guess would undo it (a chain of pending claims
            // storms this way). The node runs at its level.
            self.report.spec_shape_changes += 1;
            self.set_flag(x, F_NOSPEC, true);
            return;
        }
        self.report.runs += 1;
        self.eval_runs += 1;
        {
            let c = self.runs_in_eval.entry(x).or_insert(0);
            *c += 1;
            let c = *c;
            if c * 2 == self.limits.runs_per_node || c * 2 == self.limits.runs_per_node + 1 {
                self.tracing += 1;
            }
            if c > self.limits.runs_per_node {
                let causes = self.causes.remove(&x).unwrap_or_default();
                self.errors.push(EvalError::RunawayNode {
                    node: self.n(x).id,
                    runs: c,
                    causes,
                });
            }
        }
        if self.n(x).state == State::Parked {
            self.dequeue(x);
            self.nm(x).state = State::Dirty;
        }
        self.nm(x).on = NIL;
        if self.n(x).has(F_CLAIM_DYN) {
            // Before the children: they follow the claimed text.
            let k = r.claim.unwrap_or(0);
            self.claim_count(x, k);
        }
        {
            span!(Children);
            self.set_children(x, &r.children, r.follows, &r.child_heights, provisional);
        }
        // A stream claim may have read its own previous unfolding, which
        // the line above just replaced: those reads are gone.
        let read_freed = r.stream.is_some()
            && r.uses.iter().any(|u| {
                (u.kind == K_FIELD || u.kind == K_CHAIN)
                    && u.site != NIL
                    && (!self.live(u.site) || self.n(u.site).def_value(Field(ValueId(u.key))).is_none())
            });
        if let Some(body) = self.aux_of(x).and_then(|a| a.body) {
            for &(f, v) in &r.env {
                self.set_env_slot(body, f, v);
            }
        }
        self.set_uses(x, r.uses);
        if r.stream.is_some() {
            // A stream claim reads forward: a node it takes that reads the
            // claim back is a cycle that never parks (each run feeds the
            // next). Bound the runs per evaluation.
            let runs = self.stream_runs.entry(x).or_insert(0);
            *runs += 1;
            if *runs == STREAM_RUN_LIMIT {
                self.errors.push(EvalError::Cycle(vec![self.n(x).id]));
            }
        }
        self.set_stream(x, r.stream);
        let mut changed = {
            span!(Defs);
            self.set_defs(x, &r.defs)
        };
        if self.n(x).has(F_EFF_DYN)
            && let Some(e) = r.effect
        {
            if self.aux_of(x).and_then(|a| a.kind) != r.kind {
                self.ax(x).kind = r.kind;
            }
            self.change_effect(x, e);
        }
        if self.n(x).output != output || !self.n(x).has(F_RAN) {
            changed = true;
            self.nm(x).output = output;
            let readers = self.aux_of(x).map_or_else(Default::default, |a| a.out_readers.clone());
            for rd in readers {
                if self.tracing > 0 {
                    self.note(rd, Cause::Output { of: self.n(x).id });
                }
                self.make_dirty_quiet(rd);
            }
        }
        let _ = changed;
        self.set_publishes(x, r.publishes);
        self.set_contribs(x, r.contribs);
        match r.watch {
            Some((f, anchor, seen)) => self.set_watch(x, Some((f, anchor, seen, false))),
            None => self.drop_watch(x),
        }
        self.nm(x).sig = r.sig;
        self.set_flag(x, F_RAN, true);
        self.set_flag(x, F_REBIND, false);
        if r.speculative {
            self.spec_done.insert(x);
        } else {
            self.set_flag(x, F_NOSPEC, false);
            self.spec_done.remove(&x);
        }
        self.update_taint(x);
        if read_freed {
            count!(StaleSite);
            self.make_dirty(x);
        }
        // The run's own epoch bump (from rebinding during apply) must not
        // invalidate it.
        if self.n(x).state != State::Dirty || self.n(x).epoch != r.epoch {
            // Something it read changed while applying: it runs again.
            self.enqueue(x);
            return;
        }
        let mut pending_assumptions = 0;
        for &(t0, kind, value) in &r.assumptions {
            let mut t = t0;
            let mut kind = kind;
            if self.live(t)
                && !self.n(t).in_work()
                && let AssumeKind::Structure(f) = kind
            {
                // Resolved earlier in this batch: the prediction now rests on
                // the next pending structure (or an in-work definition).
                let (u, l) = (self.n(x).unit, self.n(x).label);
                let w = self.walk(u, l, f);
                if w.pending != NIL {
                    t = w.pending;
                } else if w.site != NIL && self.n(w.site).in_work() {
                    t = w.site;
                    kind = AssumeKind::Def(f);
                }
            }
            if self.live(t) && self.n(t).in_work() {
                pending_assumptions += 1;
                let epoch = self.n(x).epoch;
                self.ax(t).assumers.push(Assume {
                    node: x,
                    epoch,
                    kind,
                    value,
                });
            }
        }
        if pending_assumptions > 0 {
            self.report.speculations += 1;
            self.dequeue(x);
            self.nm(x).state = State::Spec;
            self.spec_live += 1;
            self.ax(x).assume_left = pending_assumptions;
            let h = self.target_height_mut(x).max(self.n(x).height);
            self.set_height(x, h);
            self.sync_order_lazy(x);
        } else {
            let h = self.target_height_mut(x);
            self.finish(x, h);
        }
    }

    /// Checks the assumptions of a speculative run whose targets are no
    /// longer in work (they finished earlier in the same batch).
    fn stale_assumptions_hold(&self, x: Slot, r: &RunResult) -> bool {
        let xn = self.n(x);
        for &(t, kind, value) in &r.assumptions {
            if !self.live(t) {
                return false;
            }
            if self.n(t).in_work() {
                continue;
            }
            let ok = match kind {
                AssumeKind::Def(f) => self.n(t).def_value(f).unwrap_or(ValueId::NONE) == value,
                AssumeKind::Output => self.n(t).output == value,
                AssumeKind::Structure(f) => {
                    let w = self.walk(xn.unit, xn.label, f);
                    let recorded = r
                        .uses
                        .iter()
                        .find(|u| u.kind == K_FIELD && u.key == f.0.0)
                        .map(|u| u.site);
                    let cur = if w.site == NIL {
                        self.start_value(xn.unit, f)
                    } else {
                        self.n(w.site).def_value(f)
                    };
                    recorded == Some(w.site)
                        && (w.pending != NIL
                            || (w.site != NIL && self.n(w.site).in_work())
                            || cur.unwrap_or(ValueId::NONE) == value)
                }
            };
            if !ok {
                return false;
            }
        }
        true
    }

    /// Whether a run's shape differs from the node's current one.
    fn shape_changes(&self, x: Slot, r: &RunResult) -> bool {
        let n = self.n(x);
        let kids: Vec<(OpId, ValueId)> = self
            .children_of(x)
            .iter()
            .map(|&c| (self.n(c).op, self.n(c).args))
            .collect();
        if kids != r.children {
            return true;
        }
        if n.has(F_CLAIM_DYN) && r.claim.unwrap_or(0) != self.aux_of(x).map_or(0, |a| a.claimed.len()) {
            return true;
        }
        if n.has(F_EFF_DYN) && r.effect.is_some_and(|e| e != n.effect) {
            return true;
        }
        if n.has(F_DYN) {
            let mut old: Vec<(Field, bool)> = n.defs.iter().map(|d| (d.field, d.global)).collect();
            let mut new: Vec<(Field, bool)> = r.defs.iter().map(|d| (d.0, d.2)).collect();
            old.sort_unstable();
            new.sort_unstable();
            return old != new;
        }
        false
    }

    /// A stopped run still unfolds: its children so far are created (so it
    /// can wait on them) and its environment set; later children are kept.
    fn apply_partial(&mut self, x: Slot, r: &RunResult) {
        if r.children.is_empty() && r.env.is_empty() {
            return;
        }
        let old = self.aux_of(x).map_or_else(Default::default, |a| a.children.clone());
        let old_specs: Vec<(OpId, ValueId)> = old.iter().map(|&c| (self.n(c).op, self.n(c).args)).collect();
        let mut specs = r.children.clone();
        let mut follows = r.follows;
        if specs.len() < old_specs.len() && old_specs[..specs.len()] == specs[..] {
            specs = old_specs;
            follows = old.iter().filter(|&&c| self.n(c).parent != x).count();
        }
        self.set_children(x, &specs, follows, &[], false);
        if let Some(body) = self.aux_of(x).and_then(|a| a.body) {
            for &(f, v) in &r.env {
                self.set_env_slot(body, f, v);
            }
        }
    }

    /// `x` completed with all reads settled (or confirmed).
    #[allow(clippy::too_many_lines)]
    fn finish(&mut self, x: Slot, height: u32) {
        span!(Finish);
        let mut work = vec![(x, height)];
        while let Some((y, h)) = work.pop() {
            if !self.live(y) {
                continue;
            }
            self.dequeue(y);
            self.leave_spec(y);
            self.nm(y).state = State::Done;
            if self.n(y).on != NIL {
                self.nm(y).on = NIL;
            }
            self.sync_pending(y);
            if self.n(y).has(F_EFF_DYN) {
                // A pending effect counts once the node is done: re-bind the
                // region if it differs from what the bindings reflect.
                let old = self.n(y).bound;
                self.sync_order(y);
                let new = self.oitem(y).eff;
                if new != old {
                    self.effect_region_changed(y, old);
                }
                self.sync_kind(y);
            }
            let old = self.n(y).height;
            if h > old {
                self.raise(y, h, y);
            } else {
                self.set_height(y, h);
            }
            self.sync_order_lazy(y);
            for c in self.children_of(y).to_vec() {
                self.clear_prov(c);
            }
            self.wake_waiters(y);
            let (u, l) = (self.n(y).unit, self.n(y).label);
            if !self.n(y).has(F_APP) && !self.units[u as usize].watches.is_empty() {
                // Range readers are checked once per batch.
                self.settle_pending.push((u, l));
            }
            // Confirm (or refute) every assumption made on `y` at once.
            let assumers = self.aux_of(y).map_or_else(Default::default, |a| a.assumers.clone());
            if assumers.is_empty() {
                continue;
            }
            self.ax(y).assumers.clear();
            for a in assumers {
                if !self.live(a.node) || self.n(a.node).epoch != a.epoch || self.n(a.node).state != State::Spec {
                    continue;
                }
                let holds = match a.kind {
                    AssumeKind::Def(f) => self.n(y).def_value(f).unwrap_or(ValueId::NONE) == a.value,
                    AssumeKind::Output => self.n(y).output == a.value,
                    AssumeKind::Structure(f) => {
                        // The pending structure resolved without re-binding
                        // the use (that would have re-dirtied the reader).
                        let rn = self.n(a.node);
                        let w = self.walk(rn.unit, rn.label, f);
                        if w.pending != NIL {
                            let epoch = rn.epoch;
                            self.ax(w.pending).assumers.push(Assume {
                                node: a.node,
                                epoch,
                                kind: a.kind,
                                value: a.value,
                            });
                            continue;
                        }
                        let cur = if w.site == NIL {
                            self.start_value(rn.unit, f)
                        } else {
                            self.n(w.site).def_value(f)
                        };
                        let recorded = rn
                            .uses
                            .iter()
                            .find(|u| u.kind == K_FIELD && u.key == f.0.0)
                            .map(|u| u.site);
                        recorded == Some(w.site)
                            && (cur.unwrap_or(ValueId::NONE) == a.value
                                || (w.site != NIL && self.n(w.site).in_work() && {
                                    let epoch = rn.epoch;
                                    self.ax(w.site).assumers.push(Assume {
                                        node: a.node,
                                        epoch,
                                        kind: AssumeKind::Def(f),
                                        value: a.value,
                                    });
                                    continue;
                                }))
                    }
                };
                if holds {
                    let left = {
                        let ax = self.ax(a.node);
                        ax.assume_left = ax.assume_left.saturating_sub(1);
                        ax.assume_left
                    };
                    if left == 0 {
                        self.report.confirmations += 1;
                        let h = self.target_height_mut(a.node);
                        work.push((a.node, h));
                    }
                } else {
                    self.make_dirty(a.node);
                }
            }
        }
    }

    // -------------------------------------------------------------- freeing

    /// Frees memoized applications that no program node reaches any more
    /// (normally done after builds, once enough have accumulated). Returns
    /// how many nodes were freed.
    pub fn collect_garbage(&mut self) -> usize {
        let before = self.live_nodes();
        self.collect_apps(true);
        before - self.live_nodes()
    }

    /// Frees the memoized applications no program node reaches (through
    /// output reads, waits, children and bodies). Run after a build, and only
    /// once the live applications have doubled since the last collection
    /// (`force`: now), so its cost is amortized over their creation.
    /// Returns whether anything was freed.
    fn collect_apps(&mut self, force: bool) -> bool {
        if !force && self.apps_live < 2 * self.apps_at_collect + 1024 {
            return false;
        }
        span!(Cycles);
        let mut reached: FxHashSet<Slot> = FxHashSet::default();
        let mut todo: Vec<Slot> = Vec::new();
        for i in 0..self.nodes.len() {
            let n = &self.nodes[i];
            if n.state != State::Free && !n.has(F_APP) && self.units[n.unit as usize].owner == NIL {
                todo.push(self.slot_at(i));
            }
        }
        while let Some(x) = todo.pop() {
            for d in self.compute_deps(x) {
                if reached.insert(d) {
                    todo.push(d);
                }
            }
        }
        let dead: Vec<Slot> = (0..self.nodes.len())
            .filter(|&i| self.nodes[i].state != State::Free && self.nodes[i].has(F_APP))
            .map(|i| self.slot_at(i))
            .filter(|a| !reached.contains(a))
            .collect();
        for &a in &dead {
            if self.live(a) {
                self.free_node(a);
            }
        }
        self.apps_at_collect = self.apps_live;
        !dead.is_empty()
    }

    /// Frees the values nothing holds any more. What the report and the
    /// error name (environment waits, waiters, causes) stays until the next
    /// evaluation, so the client can render it.
    fn sweep_values(&mut self, err: Option<&EvalError>) {
        let len = self.values.len();
        if self.values_at_sweep == 0 {
            // The first evaluation's values are nearly all live: just take
            // the baseline the table may grow to before sweeping.
            self.values_at_sweep = len;
            return;
        }
        if len < 2 * self.values_at_sweep + 4096 {
            return;
        }
        span!(Sweep);
        count!(Sweep);
        let mut live: FxHashSet<ValueId> = FxHashSet::default();
        for n in &self.nodes {
            if n.state == State::Free {
                continue;
            }
            live.insert(n.args);
            live.insert(n.output);
            for d in &n.defs {
                live.insert(d.field.0);
                live.insert(d.value);
            }
            for u in &n.uses {
                if u.kind != K_OUTPUT {
                    live.insert(ValueId(u.key));
                }
            }
        }
        for a in &self.aux {
            for &(f, v) in a.publishes.iter().chain(&a.contribs) {
                live.insert(f.0);
                live.insert(v);
            }
            for x in &a.assumers {
                live.insert(x.value);
            }
        }
        for (f, v) in self.environment.iter().chain(self.env_next.iter().flatten()) {
            live.insert(f.0);
            live.insert(*v);
        }
        live.extend(self.env_waiters.keys().map(|f| f.0));
        live.extend(self.report.env_waits.iter().map(|(_, f)| f.0));
        if let Some(e) = err {
            Self::error_fields(e, &mut live);
        }
        for u in &self.units {
            if !u.live {
                continue;
            }
            for (f, v) in &u.env {
                live.insert(f.0);
                live.insert(*v);
            }
            for w in u.watches.values() {
                live.insert(w.seen);
            }
        }
        for u in &self.units {
            if u.live {
                live.extend(u.accs.keys().map(|f| f.0));
            }
        }
        for (f, v) in self.loop_vals.iter().chain(&self.loop_init) {
            live.insert(f.0);
            live.insert(*v);
        }
        for (f, m) in &self.loop_pubs {
            live.insert(f.0);
            live.extend(m.values().copied());
        }
        for f in self.monoids.keys() {
            live.insert(f.0);
        }
        for (_, args) in self.memo.keys() {
            live.insert(*args);
        }
        live.extend(self.pins.keys().copied());
        {
            let mut tm = self.text_memo.write().unwrap_or_else(PoisonError::into_inner);
            tm.retain(|(a, b), _| live.contains(a) && live.contains(b));
            for v in tm.values() {
                live.insert(*v);
            }
            for (a, b) in tm.keys() {
                live.insert(*a);
                live.insert(*b);
            }
        }
        self.statics_live(&mut live);
        self.accs_arena_live(&mut live);
        // Built accumulator values are rebuilt on demand.
        self.acc_cache.write().unwrap_or_else(PoisonError::into_inner).clear();
        let freed = self.values.sweep(&live);
        self.report.values_freed += freed as u64;
        self.values_at_sweep = self.values.len();
    }

    /// The fields an error names.
    fn error_fields(e: &EvalError, live: &mut FxHashSet<ValueId>) {
        let waiter_fields = |ws: &[Waiter], live: &mut FxHashSet<ValueId>| {
            for w in ws {
                match w.on {
                    WaitOn::Range { field } | WaitOn::Environment(field) => {
                        live.insert(field.0);
                    }
                    WaitOn::Node(_) | WaitOn::Queued => {}
                }
            }
        };
        match e {
            EvalError::ConflictingPublication { field } | EvalError::UndeclaredDefinition { field, .. } => {
                live.insert(field.0);
            }
            EvalError::RunawayNode { causes, .. } => {
                for c in causes {
                    match c {
                        Cause::Rebind { field, .. }
                        | Cause::Changed { field, .. }
                        | Cause::Loop { field }
                        | Cause::Environment { field } => {
                            live.insert(field.0);
                        }
                        Cause::Output { .. } | Cause::Woken { .. } | Cause::Structure => {}
                    }
                }
            }
            EvalError::NoProgress { waiters, .. } | EvalError::Deadline { waiters, .. } => waiter_fields(waiters, live),
            EvalError::Cycle(_) | EvalError::NonConvergence { .. } | EvalError::RunawayCreation { .. } => {}
        }
    }

    fn accs_arena_live(&self, live: &mut FxHashSet<ValueId>) {
        self.accs_arena.for_each(&mut |item: &ValueId, _| {
            live.insert(*item);
        });
    }

    // ------------------------------------------------------------ evaluate

    /// Runs everything demanded and affected by the edits since the last
    /// evaluation, then iterates loop-back rounds to a fixpoint.
    ///
    /// # Errors
    ///
    /// Cycles, non-convergence, conflicting publications and definitions that
    /// contradict a static declaration.
    pub fn evaluate(&mut self, sched: &mut dyn Scheduler, exec: &mut dyn Executor) -> Result<Report, EvalError> {
        span!(Evaluate);
        if self.feedback {
            // Loop-back fields feed back into their own publishers: a warm
            // start could reach a different fixpoint than a cold one, so
            // follow the cold path from the initial values.
            self.restart_loops();
        }
        self.spec_done.clear();
        self.stream_runs.clear();
        self.runs_in_eval.clear();
        self.causes.clear();
        self.tracing = 0;
        self.created_in_eval.clear();
        self.idle_rounds = 0;
        self.eval_runs = 0;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.eval_start = Some(std::time::Instant::now());
        }
        self.apply_environment();
        self.touch_log.clear();
        let restarted = self.feedback;
        let mut result = self.evaluate_inner(sched, exec);
        self.feedback = self.has_feedback();
        if result.is_ok() && self.feedback && !restarted && self.restart_loops() {
            // Feedback appeared with this edit: the warm start may have
            // reached another fixpoint than the cold path. Follow the cold
            // path.
            result = self.evaluate_inner(sched, exec);
            self.feedback = self.has_feedback();
        }
        if result.is_ok() {
            self.collect_apps(false);
        }
        self.sweep_values(result.as_ref().err());
        self.finalize_report();
        let report = std::mem::take(&mut self.report);
        result.map(|()| report)
    }

    fn has_feedback(&self) -> bool {
        self.loop_pubs
            .values()
            .flat_map(|m| m.keys())
            .any(|id| self.ids.get(id).is_some_and(|&s| self.n(s).has(F_TAINT)))
    }

    /// Resets loop-back values to their initial values. Returns whether any
    /// changed.
    fn restart_loops(&mut self) -> bool {
        let mut fields: Vec<Field> = self.loop_init.keys().copied().collect();
        fields.sort_unstable();
        let mut any = false;
        for f in fields {
            let init = self.loop_init[&f];
            if self.loop_vals.insert(f, init) != Some(init) {
                self.dirty_loop_users(f);
                any = true;
            }
        }
        if any {
            self.report.restarts += 1;
        }
        any
    }

    /// Whether `s` or its unfolded text defines anything visible after it:
    /// its own definitions, a global, or a local directly in the scope `s`
    /// itself is in.
    fn defines_past(&self, s: Slot) -> bool {
        if self.n(s).defs.iter().any(|d| !self.is_hidden(d.field)) {
            return true;
        }
        let unit = self.n(s).unit;
        let root = self.units[unit as usize].root;
        let depth = self.def_depth(root, self.n(s).label, Field(ValueId::NONE));
        let mut stack: Vec<Slot> = self.children_of(s).to_vec();
        while let Some(c) = stack.pop() {
            let n = self.n(c);
            if n.state != State::Inert && !n.has(F_APP) {
                for d in n.defs.iter().filter(|d| !self.is_hidden(d.field)) {
                    if d.global || self.def_depth(root, n.label, d.field) <= depth {
                        return true;
                    }
                }
            }
            stack.extend(self.children_of(c).iter().copied());
        }
        false
    }

    fn finalize_report(&mut self) {
        let touched = std::mem::take(&mut self.touched);
        for (id, snap) in touched {
            let Some(&s) = self.ids.get(&id) else {
                if snap.existed {
                    self.report.freed += 1;
                }
                continue;
            };
            let (inert, ran, sig, args) = (
                self.n(s).state == State::Inert,
                self.n(s).has(F_RAN),
                self.n(s).sig,
                self.n(s).args,
            );
            if inert {
                continue;
            }
            if ran && self.n(s).has(F_DYN) && (!snap.existed || sig != snap.sig || args != snap.args) {
                self.report.pending_sets += 1;
                if self.defines_past(s) {
                    self.report.pending_sets_defining += 1;
                }
            }
            if !snap.existed {
                self.report.created += 1;
                if ran {
                    self.report.nodes_run += 1;
                    let name = self.op(self.n(s).op).name();
                    bump(&mut self.report.runs_by_op, name);
                }
                continue;
            }
            let sites = self.site_list(s);
            let rebound = sites.iter().filter(|x| !snap.sites.contains(x)).count();
            self.report.rebinds += rebound as u64;
            if sig != snap.sig || args != snap.args {
                self.report.nodes_run += 1;
                let name = self.op(self.n(s).op).name();
                bump(&mut self.report.runs_by_op, name);
                if self.out_hash(s) == snap.out {
                    self.report.cutoffs += 1;
                }
            }
        }
    }

    fn evaluate_inner(&mut self, sched: &mut dyn Scheduler, exec: &mut dyn Executor) -> Result<(), EvalError> {
        self.in_eval = true;
        let r = self.rounds(sched, exec);
        self.in_eval = false;
        self.touch_log.clear();
        r
    }

    /// Of `parked`, the nodes waiting for an environment field, and those
    /// waiting (through any chain of waits) on one of them: a node parked
    /// on one, or a range reader whose range holds one. A walk from the
    /// environment waiters over the waiter lists and the range readers'
    /// stabs: linear in the stalled set.
    fn env_stalled(&self, parked: &[Slot]) -> FxHashSet<Slot> {
        let parked_set: FxHashSet<Slot> = parked.iter().copied().collect();
        let mut stalled: FxHashSet<Slot> = FxHashSet::default();
        let mut todo: Vec<Slot> = self
            .env_waiters
            .values()
            .flatten()
            .copied()
            .filter(|&x| self.live(x) && self.n(x).state == State::Parked && self.n(x).on == NIL)
            .collect();
        while let Some(s) = todo.pop() {
            if !stalled.insert(s) {
                continue;
            }
            if let Some(a) = self.aux_of(s) {
                for &w in &a.waiters {
                    if parked_set.contains(&w) && self.n(w).on == s {
                        todo.push(w);
                    }
                }
            }
            let (unit, label) = (self.n(s).unit, self.n(s).label);
            for (r, _) in self.stab(unit, label) {
                if parked_set.contains(&r) && self.n(r).on == NIL {
                    todo.push(r);
                }
            }
        }
        stalled
    }

    fn rounds(&mut self, sched: &mut dyn Scheduler, exec: &mut dyn Executor) -> Result<(), EvalError> {
        loop {
            self.report.rounds += 1;
            self.drain(sched, exec)?;
            if !self.parked.is_empty() {
                let parked: Vec<Slot> = self.parked.iter().map(|&(_, s)| s).collect();
                if self.collect_apps(true) {
                    self.report.rounds -= 1;
                    continue;
                }
                // Nodes waiting for the environment, and those waiting on
                // them, stay parked until the client supplies it.
                let stalled = self.env_stalled(&parked);
                if stalled.len() == parked.len() {
                    self.report.env_stalled = stalled.len() as u64;
                    let mut waits: Vec<(NodeId, Field)> = self
                        .env_waiters
                        .iter()
                        .flat_map(|(&f, xs)| xs.iter().map(move |&x| (x, f)))
                        .filter(|&(x, _)| stalled.contains(&x))
                        .map(|(x, f)| (self.n(x).id, f))
                        .collect();
                    waits.sort_unstable();
                    self.report.env_waits = waits;
                    return Ok(());
                }
                let mut ids: Vec<NodeId> = parked
                    .iter()
                    .filter(|s| !stalled.contains(s))
                    .map(|&s| self.n(s).id)
                    .collect();
                ids.sort_unstable();
                return Err(EvalError::Cycle(ids));
            }
            let mut fields: Vec<Field> = self.loop_vals.keys().copied().collect();
            fields.sort_unstable();
            let mut changed = Vec::new();
            for f in fields {
                let Some(pubs) = self.loop_pubs.get(&f) else { continue };
                let mut vals = pubs.values().copied();
                let Some(first) = vals.next() else { continue };
                if vals.any(|v| v != first) {
                    return Err(EvalError::ConflictingPublication { field: f });
                }
                if self.loop_vals[&f] != first {
                    changed.push((f, first));
                }
            }
            if changed.is_empty() {
                return Ok(());
            }
            if self.report.rounds >= self.max_rounds {
                return Err(EvalError::NonConvergence {
                    rounds: self.report.rounds,
                });
            }
            for (f, v) in changed {
                self.loop_vals.insert(f, v);
                self.dirty_loop_users(f);
            }
        }
    }

    /// Checks a node's hinted inputs before starting it (read-only, so it
    /// runs in the parallel phase): why it must not start yet, if so.
    /// The starting height of each child `x` will unfold, computed in the
    /// parallel phase from the batch's state: above the hinted inputs as
    /// bound at `x`'s position (in-text siblings are added at commit).
    /// An estimate: it decides when a node is tried, never a result.
    pub(crate) fn child_heights(&self, x: Slot, specs: &[(OpId, ValueId)]) -> Vec<u32> {
        let n = self.n(x);
        let own = n.height + 1;
        specs
            .iter()
            .map(|&(op, args)| {
                let st = self.statics(op, args);
                let Some(h) = &st.reads else { return 0 };
                let mut height = 0;
                for &f in h {
                    if self.loop_vals.contains_key(&f) {
                        continue;
                    }
                    if n.def_value(f).is_some() || n.defs.iter().any(|d| d.field == f) {
                        height = height.max(own);
                        continue;
                    }
                    let w = self.walk(n.unit, n.label, f);
                    if w.site != NIL {
                        height = height.max(self.n(w.site).height + 1);
                    }
                    if w.pending != NIL {
                        height = height.max(self.n(w.pending).height + 1);
                    }
                }
                height
            })
            .collect()
    }

    /// The gate's walks, which the run reuses (the batch state is fixed
    /// while jobs run).
    fn gate_walks(&self, x: Slot) -> Result<Vec<(Field, WalkResult)>, End> {
        let n = self.n(x);
        if !n.has(F_HINT) || n.has(F_APP) {
            return Ok(Vec::new());
        }
        let st = self.statics(n.op, n.args);
        let Some(reads) = st.reads.as_ref() else {
            return Ok(Vec::new());
        };
        let mut walks = Vec::with_capacity(reads.len());
        for &f in reads {
            if self.loop_vals.contains_key(&f) {
                continue;
            }
            let w = self.walk(n.unit, n.label, f);
            if w.pending != NIL {
                return Err(End::Gated(w.pending));
            }
            if w.site != NIL {
                let t = self.n(w.site);
                if t.in_work() {
                    return Err(End::Gated(w.site));
                }
                if t.height >= self.level {
                    return Err(End::GateRaise(t.height + 1));
                }
            }
            walks.push((f, w));
        }
        Ok(walks)
    }

    #[allow(clippy::too_many_lines)]
    fn drain(&mut self, sched: &mut dyn Scheduler, exec: &mut dyn Executor) -> Result<(), EvalError> {
        let mut ready = Vec::new();
        let mut cands = Vec::new();
        let mut batch = Vec::new();
        let mut results = Vec::new();
        loop {
            self.flush_claims();
            if let Some(e) = self.errors.first().cloned() {
                self.errors.clear();
                return Err(e);
            }
            self.settle_deferred();
            self.flush_order();
            let Some((level, at_level)) = self.queue_first() else {
                return Ok(());
            };
            if self.past_deadline() {
                return Err(EvalError::Deadline {
                    runs: self.eval_runs,
                    waiters: self.waiters(),
                });
            }
            self.level = level;
            ready.clear();
            #[cfg(feature = "profile")]
            let gate_span = crate::obs::enter_impl(crate::obs::Span::Gate);
            for s in at_level {
                if self.n(s).state == State::Dirty && self.n(s).has(F_REBIND) && self.revalidate(s) {
                    continue;
                }
                if self.n(s).state == State::Dirty {
                    ready.push(s);
                }
            }
            #[cfg(feature = "profile")]
            drop(gate_span);
            if ready.is_empty() {
                continue;
            }
            #[cfg(feature = "profile")]
            let select_span = crate::obs::enter_impl(crate::obs::Span::Select);
            cands.clear();
            let budget = sched.speculation_budget();
            let room = sched.max_outstanding().saturating_sub(self.spec_live as usize);
            if budget > 0 && room > 0 {
                let lim = budget.min(room);
                let mut seen = FxHashSet::default();
                'levels: for h in (level as usize + 1)..self.queue.len() {
                    for &(id, s) in &self.queue[h] {
                        if cands.len() >= lim {
                            break 'levels;
                        }
                        if !self.queue_live(h, id, s) || !seen.insert(s) {
                            continue;
                        }
                        let n = self.n(s);
                        // Re-bound nodes are revalidated at their level instead
                        // (cheaper than any run).
                        if n.state == State::Dirty && !n.has(F_NOSPEC) && !n.has(F_REBIND) {
                            cands.push(Candidate {
                                node: n.id,
                                slot: s,
                                has_prediction: n.has(F_RAN) || n.has(F_PREDICT),
                                waiting: false,
                            });
                        }
                    }
                }
                for &(_, s) in &self.parked {
                    if cands.len() >= 2 * lim {
                        break;
                    }
                    let n = self.n(s);
                    if !n.has(F_NOSPEC) && n.on != NIL {
                        cands.push(Candidate {
                            node: n.id,
                            slot: s,
                            has_prediction: n.has(F_RAN) || n.has(F_PREDICT),
                            waiting: true,
                        });
                    }
                }
            }
            let mut ready_ids: Vec<(NodeId, Slot)> = ready.iter().map(|&s| (self.n(s).id, s)).collect();
            ready_ids.sort_unstable();
            let just_ids: Vec<NodeId> = ready_ids.iter().map(|r| r.0).collect();
            batch.clear();
            sched.select(&just_ids, &cands, &mut batch);
            Self::sanitize(&ready_ids, &cands, &mut batch);
            #[cfg(feature = "profile")]
            drop(select_span);
            self.report.steps += 1;
            count!(Step);
            count!(Job, batch.len());
            count!(SpecJob, batch.iter().filter(|j| j.speculative).count());
            let fuel = sched.fuel();
            results.clear();
            {
                span!(RunBatch);
                let this: &Engine = self;
                let jobs = &batch;
                exec.execute(jobs.len(), &|i| this.run_job(jobs[i], fuel), &mut results);
            }
            span!(ApplyBatch);
            results.sort_by_key(|r| r.node);
            let runs_before = self.report.runs;
            for r in results.drain(..) {
                self.apply(r);
            }
            self.settle_deferred();
            if self.report.runs == runs_before {
                self.idle_rounds += 1;
                if self.idle_rounds > self.limits.idle_rounds {
                    return Err(EvalError::NoProgress {
                        rounds: self.idle_rounds,
                        waiters: self.waiters(),
                    });
                }
            } else {
                self.idle_rounds = 0;
            }
        }
    }

    /// Whether the evaluation's run or time limit is reached.
    fn past_deadline(&self) -> bool {
        let runs = self.limits.total_runs.is_some_and(|m| self.eval_runs >= m);
        #[cfg(not(target_arch = "wasm32"))]
        let time = matches!((self.limits.time, self.eval_start), (Some(t), Some(s)) if s.elapsed() >= t);
        #[cfg(target_arch = "wasm32")]
        let time = false;
        runs || time
    }

    fn sanitize(ready: &[(NodeId, Slot)], cands: &[Candidate], batch: &mut Vec<Job>) {
        let is_ready = |x: &NodeId| ready.binary_search_by_key(x, |r| r.0).is_ok();
        batch.retain(|j| is_ready(&j.node) || (j.speculative && cands.iter().any(|c| c.node == j.node)));
        batch.sort_by_key(|j| (j.node, j.speculative));
        batch.dedup_by_key(|j| j.node);
        for j in batch.iter_mut() {
            if j.slot == NIL {
                j.slot = ready.binary_search_by_key(&j.node, |r| r.0).map_or_else(
                    |_| cands.iter().find(|c| c.node == j.node).map_or(NIL, |c| c.slot),
                    |i| ready[i].1,
                );
            }
        }
        if !batch.iter().any(|j| !j.speculative && is_ready(&j.node))
            && let Some(&(id, slot)) = ready.first()
        {
            match batch.iter_mut().find(|j| j.node == id) {
                Some(j) => j.speculative = false,
                None => batch.push(Job {
                    node: id,
                    slot,
                    speculative: false,
                }),
            }
        }
    }

    pub(crate) fn run_job(&self, job: Job, fuel: u64) -> RunResult {
        span!(Job);
        let n = self.n(job.slot);
        // Borrowed, not cloned: every worker would bump the same count.
        let op = &**self.op(n.op);
        let mut cx = Ctx::new(self, job.slot, job.speculative, fuel);
        if !job.speculative {
            match self.gate_walks(job.slot) {
                Ok(walks) => cx.walks = walks,
                Err(e) => {
                    let r = cx.stop(e);
                    return cx.finish(r);
                }
            }
        }
        let r = op.run(&mut cx);
        cx.finish(r)
    }
}

pub(crate) fn eff_code(e: Effect) -> u8 {
    match e {
        Effect::Open => EFF_OPEN,
        Effect::Close => EFF_CLOSE,
        Effect::None | Effect::Pending => EFF_NONE,
        Effect::Scope(b) => {
            assert!(b.closes <= 3 && b.opens <= 3, "at most 3 brackets each way");
            b.closes | (b.opens << 2) | if b.before { EFF_BEFORE } else { 0 }
        }
    }
}

/// The kind an effect gives the scope it opens.
pub(crate) fn eff_kind(e: Effect) -> Option<ValueId> {
    match e {
        Effect::Scope(b) => b.kind,
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub(crate) enum End {
    Output(ValueId),
    Wait(Slot),
    /// Waiting for an environment field neither supplied nor absent.
    WaitEnv(Field),
    Raise(u32),
    NeedApp(OpId, ValueId),
    NeedChild(usize),
    WaitRange(Field, Slot),
    OutOfFuel,
    Unpredictable,
    /// Not started: a hinted input is in work (or behind pending structure).
    Gated(Slot),
    /// Not started: a hinted input is finished but not settled below `h`.
    GateRaise(u32),
}

/// What one run of a node reported. Opaque to clients.
pub struct RunResult {
    pub(crate) node: NodeId,
    pub(crate) slot: Slot,
    pub(crate) epoch: u32,
    pub(crate) speculative: bool,
    pub(crate) end: End,
    pub(crate) uses: Vec<Use>,
    pub(crate) defs: Vec<(Field, ValueId, bool)>,
    pub(crate) children: Vec<(OpId, ValueId)>,
    /// How many at the end of `children` are continuations (at the node's
    /// own depth).
    pub(crate) follows: usize,
    /// The starting height of each child (see [`Engine::child_heights`]),
    /// or empty when not computed.
    pub(crate) child_heights: Vec<u32>,
    pub(crate) env: Vec<(Field, ValueId)>,
    pub(crate) publishes: Vec<(Field, ValueId)>,
    pub(crate) contribs: Vec<(Field, ValueId)>,
    pub(crate) watch: Option<(Field, Slot, ValueId)>,
    pub(crate) effect: Option<u8>,
    /// The kind of the scope the run's effect opens.
    pub(crate) kind: Option<ValueId>,
    pub(crate) claim: Option<usize>,
    pub(crate) stream: Option<StreamRange>,
    /// The length of `touch_log` when the run started.
    pub(crate) touch_seq: usize,
    /// `Engine::relabels` when the run started.
    pub(crate) relabels: u64,
    pub(crate) assumptions: Vec<(Slot, AssumeKind, ValueId)>,
    pub(crate) sig: u64,
    pub(crate) error: Option<EvalError>,
    /// Unfolded-text and lexical-application memo hits and misses.
    pub(crate) memo: [u32; 4],
}

impl Node {
    pub(crate) fn has_ran(&self) -> bool {
        self.has(F_RAN)
    }

    pub(crate) fn is_app(&self) -> bool {
        self.has(F_APP)
    }

    pub(crate) fn is_dyn(&self) -> bool {
        self.has(F_DYN)
    }

    pub(crate) fn is_claim_dyn(&self) -> bool {
        self.has(F_CLAIM_DYN)
    }
}

impl Engine {
    pub(crate) fn child_ids_pub(&self, x: Slot, specs: &[(OpId, ValueId)]) -> Vec<NodeId> {
        self.child_ids(x, specs)
    }

    pub(crate) fn slot_by_id(&self, id: NodeId) -> Option<Slot> {
        self.ids.get(&id).copied()
    }

    /// (in-work count, max height) over `[lo, hi)` of a unit.
    pub(crate) fn order_fold(&self, unit: u32, lo: u64, hi: u64) -> (u32, u32) {
        let s = self.ord_fold(unit, lo, hi);
        (s.work, s.maxh)
    }

    pub(crate) fn prev_sibling_where(&self, x: Slot, pred: &dyn Fn(OpId, &[ValueId]) -> bool) -> Option<NodeId> {
        let n = self.n(x);
        let root = self.units[n.unit as usize].root;
        let mut label = n.label;
        while let Some((k, i)) = self.order.prev(root, label) {
            label = k;
            let s = i.slot;
            let m = self.n(s);
            if m.parent == n.parent {
                // A sibling a claim took is not there for anyone.
                if m.state != State::Inert && pred(m.op, &self.args_vec(m.args)) {
                    return Some(m.id);
                }
            } else if !self.descends(s, n.parent) {
                return None;
            }
        }
        None
    }
}
