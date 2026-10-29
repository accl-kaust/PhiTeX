//! The client's types: ids, the operation trait, the small enums, reports.

use std::sync::Arc;
use std::time::Duration;

use crate::values::{Field, ValueId, Values};

/// An operation registered with the engine.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct OpId(pub(crate) u32);

/// A node: one application of an operation at a position.
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

/// A unit: a contiguous range of the line, made at once, replaced as one on
/// an edit of its text. Units follow each other in the order they are made,
/// or after a node (`Engine::new_unit_after`).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct UnitId(pub(crate) u32);

/// Where a definition is seen: its scope depth. Scopes are brackets on
/// the line (`Effect`): a node closes some before itself and opens some
/// after itself, and the depth at a position is the count of scopes open
/// there. A definition at depth `k` reaches a later position only while
/// no close has brought the depth below `k` in between; a `Global` one
/// (depth 0) reaches every later position until the next definition.
/// `Local`: the depth at the node after its closes and before its opens
/// (its end state, outside the scopes it leaves open). `Scope`: inside
/// the scopes it opens (that depth plus its opens). `ScopeAt(k)`: inside
/// the `k`-th scope it opens (0: the outermost).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub enum Locality {
    Local,
    Global,
    Scope,
    ScopeAt(u8),
}

/// What an operation may define, for prediction only: never for waiting.
#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub enum MayDefine {
    Nothing,
    Any,
    Names(Vec<Field>),
    Prefixes(Vec<String>),
}

/// A node's scope brackets: it closes `closes` scopes and opens `opens`.
/// With `before`, the closes come before the node's reads and
/// definitions (a node whose text begins by closing a group); without,
/// after its definitions. The opens always come after its definitions.
/// What a scope is to the client is a definition inside it
/// (`Locality::Scope`), read by depth like any other.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct Brackets {
    pub closes: u8,
    pub opens: u8,
    pub before: bool,
}

/// A node's scope effect: `Open` opens one scope after itself, `Close`
/// closes one before itself, `Scope` as its brackets say, `Pending`
/// means the run will say (`Ctx::set_effect`). An operation's `effect`
/// is filed when the node is made, so readers after it see the right
/// depth before it runs; a run that sets another re-asks the readers
/// after it whose answer moved.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub enum Effect {
    None,
    Open,
    Close,
    Scope(Brackets),
    Pending,
}

impl Effect {
    /// As brackets (`Pending` as none).
    #[must_use]
    pub fn brackets(self) -> Brackets {
        match self {
            Effect::None | Effect::Pending => Brackets::default(),
            Effect::Open => Brackets {
                opens: 1,
                ..Brackets::default()
            },
            Effect::Close => Brackets {
                closes: 1,
                before: true,
                ..Brackets::default()
            },
            Effect::Scope(b) => b,
        }
    }
}

/// Returned (via `?`) by a read when the run is already refuted: a commit
/// changed something it read earlier in this same run. The engine runs the
/// node again; the operation just returns. Nothing ever waits.
#[derive(Debug)]
pub struct Stop {
    pub(crate) _private: (),
}

/// Where a read looks (`Ctx::read_at`).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum At {
    /// The definition reaching `n`: the last before its position.
    Before(NodeId),
    /// `n`'s own definition if it makes one, else the one reaching it.
    AfterOwn(NodeId),
    /// The definition reaching the end of the unit: the last one in it.
    End(UnitId),
    /// The last definition on the whole line, whatever comes after the
    /// reader: a loop-back read (a label read at the front of a document,
    /// defined at its back). It is answered the loop-back value as of the
    /// last quiet point: the previous complete pass, as TeX's own passes
    /// read the aux file the last run left, never a part of what this pass
    /// is writing (a field none published yet: its prediction, else the
    /// environment's). A quiet point is an event, the count of queued and
    /// running nodes reaching zero, never something a worker waits for:
    /// there every loop-back definition is published again
    /// (`Operation::read_back`'s together) and only the readers whose
    /// answer moved run again, the next pass. `Limits::final_reruns`
    /// counts passes: a reader re-asked in more of them in one evaluation
    /// is left as it is and listed in `Report::capped` (a document that
    /// does not converge).
    Final,
}

/// A read at another node's position (`Ctx::read_at`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FieldAt {
    /// The definition reaching it: its value and its defining node (`None`
    /// for the environment's value).
    Def(ValueId, Option<NodeId>),
    /// No definition and no environment value reaches it.
    Unset,
    /// The node named is gone: the reader is re-asked when it comes back.
    Gone,
}

/// A projection registered with `Engine::register_projection`, read through
/// a one-step path: a reader depends on the projected value only.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct ProjId(pub(crate) u16);

impl ProjId {
    /// `true` for an empty, zero, nil or unit value, `false` otherwise.
    pub const IS_EMPTY: ProjId = ProjId(1);
}

/// One step of a read path. A step that does not apply (an index past the
/// end, a key not there, another kind of value) gives nothing, and the
/// path's answer is none. `Each`, `Indices`, `Keys`, `Rank`, `Ranks` and
/// `Distinct` are refused by this core (`unimplemented!("path step: next
/// run")`).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub enum Step {
    /// Element `i` of a list or a sequence.
    Index(u32),
    /// The last element of a list or a sequence.
    Last,
    /// The length of a list, a sequence (NIL: 0) or a string (bytes).
    Len,
    /// The value for key `k` of an association list (a list of lists
    /// `[k, v, ..]`: the first whose head is `k`).
    Key(ValueId),
    /// Whether an association list has key `k`.
    Has(ValueId),
    Proj(ProjId),
    /// Whether there is a value.
    Present,
    /// Whether the value is `k`.
    Eq(ValueId),
    /// The order of an integer against `k`: -1, 0 or 1.
    Cmp(ValueId),
    Each(PathId),
    Indices(ValueId),
    Keys(ValueId),
    Rank(ValueId),
    Ranks(ValueId),
    Distinct,
}

/// A read path: steps applied in turn to a value read. A reader through a
/// path depends only on the path's answer. `WHOLE` is the value itself.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct PathId(pub(crate) u32);

impl PathId {
    pub const WHOLE: PathId = PathId(0);

    /// Projection `p` as a path.
    #[must_use]
    pub fn of_proj(p: ProjId) -> PathId {
        PathId(u32::from(p.0))
    }
}

/// A client projection: pure, from a value to a small value.
pub type Projection = Arc<dyn Fn(ValueId, &Values) -> ValueId + Send + Sync>;

/// The environment's value of a field, on demand: `None` when the client
/// cannot say, `Some(None)` for absent.
pub type EnvProvider = dyn Fn(&Values, Field) -> Option<Option<ValueId>> + Send + Sync;

/// What the engine runs. It is pure in its reads: two runs with the same
/// reads make the same definitions, contributions, placements and output.
pub trait Operation: Send + Sync {
    fn name(&self) -> &'static str;

    /// The run. Reads may return `Stop`; then return it.
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop>;

    /// What a run may define, for prediction (default: anything).
    fn may_define(&self, _args: &[ValueId], _values: &Values) -> MayDefine {
        MayDefine::Any
    }

    /// Definitions filed for the node before it runs (a record of its last
    /// build): readers after it see them until its run replaces them, and
    /// a run that makes the same ones re-asks nobody.
    fn predicted_defs(&self, _args: &[ValueId], _values: &Values) -> Option<Vec<(Field, ValueId)>> {
        None
    }

    /// What a node of this operation with `args` will define as a
    /// relation on the value reaching it, before it runs (a counter the
    /// text steps: `\stepcounter{X}` predicts +1 on X's register): filed
    /// as provisional relations when the node is made, as a learned shape
    /// is (`Ctx::define_rel` by the run replaces them; a run that defines
    /// the field otherwise, or not at all, refutes only the readers whose
    /// answer moves). `None`: no prediction. A field `predicted_defs`
    /// gives a value for keeps that value.
    fn predicted_rels(&self, _args: &[ValueId], _values: &Values) -> Option<Vec<(Field, i64)>> {
        None
    }

    /// Whether what its runs define is read back (`At::Final`: a file the
    /// job writes and reads): all of it published together at each quiet
    /// point (`commit::publish_finals`), so a pass reads one whole state
    /// of what the pass before wrote, a field and the fields it names
    /// alike (default no: a field of another node is published where it
    /// is first read back).
    fn read_back(&self) -> bool {
        false
    }

    /// Whether its nodes run only once the line is quiet (default no):
    /// one made dirty during an evaluation waits, uncounted, until no other
    /// node is queued or running, and then runs before the loop-back values
    /// are published (a pass's writers: a page's PDF side, the link, the
    /// files): so they run once a pass, not once for every rerun of what
    /// they read. Nothing an evaluation needs may wait on one.
    fn final_only(&self) -> bool {
        false
    }

    /// Whether its nodes' definitions settle at quiet points (default no;
    /// docs/spec-settle.md). A change of a definition by such a node
    /// re-asks a reader of another such operation at once, and any other
    /// reader only at the next quiet point, its read checked again then
    /// (`sched::hold_wake`): the page layer converges among itself inside
    /// a pass, and the text sees one state of it per pass, as TeX's own
    /// passes read the aux file the run before wrote, never a part of
    /// what this one is writing. A run that says its definitions are no
    /// decision of the layer (`Ctx::settle_now`) re-asks every reader at
    /// once.
    fn settles_at_quiet(&self) -> bool {
        false
    }

    /// Whether its nodes define alike (default yes): a relation one of
    /// them makes (`Ctx::define_rel`) is then predicted for the others
    /// that have not run (`commit::learn_shape`). An operation whose nodes
    /// each run their own text says no: one node's counter step predicts
    /// nothing of another's.
    fn alike(&self) -> bool {
        true
    }

    /// Whether its nodes are siblings to a claim (`Ctx::claim`,
    /// `Ctx::following`; default yes): a node set among the text's pieces
    /// to start work beside them says no, and a claim passes over it.
    fn sibling(&self) -> bool {
        true
    }

    /// Whether its nodes are pieces of the unit's text, the rest of which
    /// a run's `Ctx::claim_rest` takes (default no): what a client sets
    /// beside or after the text to work on it (its marks, writers, links)
    /// is not text, and a claim of the rest passes over it. A claim that
    /// took them wrote no output after a fatal error, where TeX writes the
    /// pages shipped before it.
    fn text(&self) -> bool {
        false
    }

    /// Its scope effect, if known before it runs.
    fn effect(&self, _args: &[ValueId], _values: &Values) -> Effect {
        Effect::None
    }

    /// The values the client holds for this operation outside the engine
    /// (a cache it decodes again, a run state kept for a resume), each
    /// given to `keep`, at a collection ([`crate::Engine::collect`]):
    /// what it does not give may be freed. A cache keyed by values may be
    /// emptied here instead. With nothing running.
    fn roots(&self, _eng: &crate::Engine, _keep: &mut dyn FnMut(ValueId)) {}

    /// Whether a run may contribute to `f` (default: yes). A node that has
    /// not committed and never contributes `f` is passed over by
    /// `Ctx::next_contribution`, not a stop.
    fn contributes(&self, _args: &[ValueId], _values: &Values, _f: Field) -> bool {
        true
    }
}

use crate::ctx::Ctx;

/// How contributions fold (`Engine::declare_accumulator`). `combine` is
/// associative; the engine keeps a sum per segment and combines sums.
pub trait Monoid: Send + Sync {
    fn empty(&self, values: &Values) -> ValueId;
    fn combine(&self, values: &Values, a: ValueId, b: ValueId) -> ValueId;
}

/// Sequence concatenation: the default fold.
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

/// Why a node ran again (`Report::rerun_causes`, `Engine::critical_chain`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cause {
    /// A read of `field` now reaches another definition.
    Rebind {
        field: Field,
        from: Option<NodeId>,
        to: Option<NodeId>,
    },
    /// The definition of `field` at `site` (the environment: `None`)
    /// changed its value.
    Changed { field: Field, site: Option<NodeId> },
    /// A fold it reads over `field` changed, last by `by`'s contribution.
    Range { field: Field, by: NodeId },
    /// The output of `of`, which it reads, changed.
    Output { of: NodeId },
    /// The environment's `field` changed.
    Environment { field: Field },
    /// The structure around it changed: a claim, a node it named gone.
    Structure,
    /// Its creation: placed by `by` (`None`: pushed by the client).
    Created { by: Option<NodeId> },
    /// The client replaced its text.
    Edited,
}

/// What `Engine::take_settled` reports for a marked node.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Settled {
    pub node: NodeId,
    pub version: u64,
    pub output: ValueId,
}

/// A node as the client sees it (`Engine::node_info`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo {
    pub op: &'static str,
    pub args: Vec<ValueId>,
    pub unit: UnitId,
    /// The node whose run placed it, if any.
    pub parent: Option<NodeId>,
    pub state: NodeState,
    /// The node whose claim holds it, if one does.
    pub claimer: Option<NodeId>,
}

/// A node's state, coarsely.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NodeState {
    /// To run, or running.
    Dirty,
    /// Ran, and everything it read still reaches it.
    Done,
    /// Taken by a claim: never runs.
    Inert,
}

/// Where the evaluation's time went: nanoseconds, summed over workers
/// except `wall_ns`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct Times {
    pub wall_ns: u64,
    /// In operations' runs, the reads inside included.
    pub op_ns: u64,
    /// Of `op_ns`, in runs that were not their node's last in this
    /// evaluation: thrown away.
    pub wasted_ns: u64,
    /// In commits: the diff into the node's own segment.
    pub commit_ns: u64,
    pub commit_max_ns: u64,
    /// In the walks forward that re-ask readers.
    pub reask_ns: u64,
    /// Workers with nothing to run and nothing to steal.
    pub idle_ns: u64,
    /// Waiting for any lock (`lock`).
    pub lock_wait_ns: u64,
    pub lock_wait_max_ns: u64,
}

/// What an evaluation did.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Distinct nodes that ran.
    pub nodes_run: u64,
    /// Runs in all: a node run again counts each time.
    pub runs: u64,
    pub runs_by_op: Vec<(&'static str, u64)>,
    /// Nodes the runs' placements made (a placement kept makes none), by
    /// operation.
    pub placed_by_op: Vec<(&'static str, u64)>,
    /// The commits by operation: their whole time in ns (under the lock
    /// and the re-asks after) and the reads their records file.
    pub commit_by_op: Vec<(&'static str, u64, u64)>,
    /// Nodes that ran once and were never refuted.
    pub first_runs: u64,
    /// The summed run time of every run that was not its node's last.
    pub wasted_ns: u64,
    /// Nodes run again other than by resumptions: each a refuted node.
    pub refuted_nodes: u64,
    /// Runs that went on from where their node's last run parked
    /// (`Ctx::parked_on`, `Ctx::goes_on`), woken by the answer it parked
    /// on: TeX's page builder going on after each paragraph, not a rerun.
    pub resumed: u64,
    /// Commits of runs that parked on an answer not in yet.
    pub parks: u64,
    /// Runs stopped at a check (`Ctx::tick`) because a read moved while
    /// they ran: run again on what it moved to.
    pub stopped_stale: u64,
    /// Runs that panicked on inputs that moved while they ran, run again
    /// (a panic on inputs that still hold is `EvalError::OpPanicked`).
    pub stale_panics: u64,
    pub refuted_by_field: Vec<(Field, u64)>,
    pub rerun_causes: Vec<(NodeId, Cause)>,
    /// With `profile`: every run's time by node.
    pub run_times: Vec<(NodeId, Duration)>,
    /// The longest chain of nodes each woken by the previous one's commit
    /// (`Engine::critical_chain`): its length and its summed run time.
    pub realised_critical_path: u32,
    pub realised_critical_time: Duration,
    /// Predicted definitions (`Operation::predicted_defs`) a run confirmed
    /// or replaced.
    pub predictions_confirmed: u64,
    pub predictions_refuted: u64,
    /// [`Report::predictions_refuted`] by field, most first.
    pub predictions_refuted_by_field: Vec<(Field, u64)>,
    /// Relative definitions recomputed from their input without a run.
    pub relative_resolved: u64,
    /// Nodes given a relation ahead of their run from their operation's
    /// shape (`commit::learn_shape`).
    pub shaped: u64,
    pub commits: u64,
    /// The node whose commit held its segment longest
    /// (`Times::commit_max_ns`).
    pub commit_max_node: Option<NodeId>,
    pub times: Times,
    pub segments: u32,
    pub steals: u64,
    pub version: u64,
    pub edits: u64,
    /// Readers through `At::Final` re-asked past `Limits::final_reruns`:
    /// left with their last answer (the client's "labels may have
    /// changed").
    pub capped: Vec<NodeId>,
    /// The quiet points at which a loop-back value moved and its readers
    /// ran again: the passes a loop-back took to settle beyond the first.
    pub final_reruns: u32,
    /// The passes the evaluation made: one, and one more for each quiet
    /// point whose published values re-asked a reader (a LaTeX build's
    /// runs).
    pub passes: u32,
    /// The runs each pass made, in order (their sum is `runs`).
    pub pass_runs: Vec<u64>,
    /// The edits taken during the evaluation (`EditSource::absorb`): each
    /// one's version and the runs made before it.
    pub absorbed: Vec<(u64, u64)>,
    /// The reader records of earlier runs dropped at the evaluation's
    /// quiet points.
    pub readers_swept: u64,
    /// Placements relocated instead of remade (docs/spec-continuations.md
    /// §2.3): a kept node moved to the place its placer wants now.
    pub placements_moved: u64,
    /// [`Report::placements_moved`] by the relocated node's operation.
    pub placements_moved_by_op: Vec<(&'static str, u64)>,
    /// Wakes held to a quiet point (`Operation::settles_at_quiet`): those
    /// deferred, those that re-asked their reader there (the rest found
    /// the read holding), and the quiet points at which any did (the
    /// passes of the page layer's feedback).
    pub held_deferred: u64,
    pub held_fired: u64,
    pub held_passes: u32,
    /// Held wakes were dropped past `Limits::settle_passes`: the page
    /// layer did not settle; the output is the last pass's.
    pub held_unsettled: bool,
    /// Runs whose commit found every output of the node's run before
    /// equal (its definitions and relations, contributions, output,
    /// brackets and placements): reruns that asked nothing again, the
    /// runs an exact engine would not have made; and by operation.
    pub runs_wasted: u64,
    pub wasted_by_op: Vec<(&'static str, u64)>,
    /// The collections the evaluation made (`Limits::collect_at_bytes`):
    /// what each freed, and the nanoseconds each held the workers.
    pub collections: Vec<(crate::Collected, u64)>,
}

impl Report {
    #[must_use]
    pub fn by_op(list: &[(&'static str, u64)], name: &str) -> u64 {
        list.iter().filter(|(n, _)| *n == name).map(|(_, k)| *k).sum()
    }
}

/// Bounds on an evaluation.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Runs of one node in one evaluation (default 1 << 20, a sanity
    /// bound: a node is re-asked by whatever lands; one that runs again
    /// on unchanged reads is what `Engine::check_invariants` finds),
    /// `EvalError::RunawayNode`.
    pub runs_per_node: u32,
    /// The passes of one evaluation a node is re-asked in through
    /// `At::Final` (default 10, about what a LaTeX user tolerates in
    /// passes): past it the node is capped (`Report::capped`), never an
    /// error.
    pub final_reruns: u32,
    /// The quiet points of one build at which held wakes may fire
    /// (`Operation::settles_at_quiet`): past it the wakes still held are
    /// dropped, the last state is written out, and `Report::held_unsettled`
    /// says so, for the client to report as latexmk reports "maximum runs
    /// reached" (the output kept). latexmk's `max_repeat` is 5 whole runs;
    /// a pass here is finer and at eight threads a book's chapters take
    /// 8 to 12 of them while the text still speculates ahead of the page
    /// layer, so the default is 12 until that is settled (measured: 5 made
    /// every course3 -j8 cold build end unsettled and wrong).
    pub settle_passes: u32,
    /// Runs in the whole evaluation (default none).
    pub total_runs: Option<u64>,
    /// Wall time (default none).
    pub time: Option<Duration>,
    /// How long a build may make no progress before it is stuck (default
    /// none): a run that reports no progress for this long
    /// (`Ctx::stuck`, the client's measure), or every worker idle for a
    /// sixth of it with nodes still outstanding (a wake lost), ends the
    /// evaluation with `EvalError::Stuck`, the nodes queued pending.
    pub stall: Option<Duration>,
    /// The process's resident memory past which a run reports itself
    /// stuck (default none; the client measures it, `Ctx::stuck`).
    pub memory_bytes: Option<u64>,
    /// The value store's payload at which the evaluation stops every
    /// worker at the end of its run and collects (`Engine::collect`;
    /// default none): then at twice what the last collection left, if
    /// more.
    pub collect_at_bytes: Option<u64>,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            runs_per_node: 1 << 20,
            final_reruns: 10,
            settle_passes: 12,
            total_runs: None,
            time: None,
            stall: None,
            memory_bytes: None,
            collect_at_bytes: None,
        }
    }
}

#[derive(Debug)]
pub enum EvalError {
    RunawayNode {
        node: NodeId,
        op: &'static str,
        runs: u32,
    },
    OpPanicked {
        node: NodeId,
        op: &'static str,
        message: String,
    },
    TotalRuns {
        runs: u64,
    },
    Deadline,
    /// The build stopped making progress (`Limits::stall`): a run that
    /// said so (`Ctx::stuck`, its node and why), or the workers idle with
    /// work outstanding (no node).
    Stuck {
        node: Option<NodeId>,
        op: &'static str,
        why: String,
    },
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::RunawayNode { node, op, runs } => write!(f, "node {} ({op}) ran {runs} times", node.0),
            EvalError::OpPanicked { node, op, message } => write!(f, "node {} ({op}) panicked: {message}", node.0),
            EvalError::TotalRuns { runs } => write!(f, "{runs} runs in all"),
            EvalError::Deadline => write!(f, "deadline"),
            EvalError::Stuck { node: Some(n), op, why } => write!(f, "stuck: node {} ({op}): {why}", n.0),
            EvalError::Stuck { node: None, why, .. } => write!(f, "stuck: {why}"),
        }
    }
}

impl std::error::Error for EvalError {}

/// Edits absorbed during an evaluation (`Engine::evaluate_absorbing`): the
/// engine asks for edits whenever the pool is quiescent and, between, every
/// few milliseconds from the caller's thread; `Some(version)` means some
/// were applied (more dirty nodes), `None` at quiescence ends.
pub trait EditSource {
    fn absorb(&mut self, eng: &crate::Engine) -> Option<u64>;

    /// Marked nodes settled at a version; `true` to stop early.
    fn settled(&mut self, _eng: &crate::Engine, _items: &[Settled]) -> bool {
        false
    }

    /// The values it holds for edits not yet taken (a text interned
    /// before it is applied), each given to `keep`, at a collection
    /// during the evaluation (`Limits::collect_at_bytes`): what it does
    /// not give may be freed.
    fn roots(&self, _keep: &mut dyn FnMut(ValueId)) {}
}

pub struct NoEdits;

impl EditSource for NoEdits {
    fn absorb(&mut self, _eng: &crate::Engine) -> Option<u64> {
        None
    }
}
