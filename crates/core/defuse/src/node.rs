//! A node and what its last run left.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use crate::api::{Brackets, Cause, Effect, Locality, NodeId, OpId, PathId, UnitId};
use crate::index::RangeKey;
use crate::lock::{Lock, Mx};
use crate::order::{Pos, SegId};
use crate::segment::RangeReader;
use crate::values::{Field, ValueId};

/// A node's state: one byte, changed by compare-and-swap, so a re-ask never
/// takes a lock on the node.
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum State {
    /// Ran; nothing it read has changed since.
    Clean = 0,
    /// To run: on a worker's deque or the injector.
    Dirty = 1,
    Running = 2,
    /// Re-asked while running: its run commits, then it runs again. Its
    /// reads return `Stop` from the moment it is marked.
    RunningDirty = 3,
    /// Taken by a claim.
    Inert = 4,
    /// Removed: unfiled, never run again.
    Gone = 5,
    /// Taken by a claim while a run of it is in progress: its reads return
    /// `Stop`, its commit files nothing, and its worker makes it `Inert`;
    /// given back meanwhile, it is `RunningDirty`, run again by that worker,
    /// never by a second one beside it.
    RunningInert = 6,
}

pub(crate) struct Node {
    pub(crate) id: NodeId,
    pub(crate) unit: UnitId,
    /// Its operation and arguments: its text, swapped by an edit
    /// (`Engine::replace`), cloned by a read.
    pub(crate) op: AtomicU32,
    pub(crate) text: Lock<Arc<[ValueId]>>,
    pub(crate) parent: Option<NodeId>,
    /// Its segment and label, written under the owning segment's write
    /// lock (a relabel, a split), read under its read lock. A lock-free read
    /// (`pos_hint`) is good enough to find the segment to lock.
    pub(crate) seg: AtomicU32,
    pub(crate) pos: AtomicU64,
    pub(crate) state: AtomicU8,
    /// Its commit has filed its reads and answered them under its lock and
    /// it is still `Running`: a walk that finds one of them changed marks it
    /// as `sched::mark_dirty` does.
    pub(crate) committing: AtomicBool,
    /// Runs in this evaluation (`Limits::runs_per_node`), the evaluation
    /// they count in.
    pub(crate) runs: AtomicU32,
    pub(crate) run_epoch: AtomicU64,
    /// Bumped at each commit; a reader record carrying an older serial is
    /// stale and dropped when met, so a commit never visits other segments
    /// to retract its node's old reads. Zero: never committed.
    pub(crate) serial: AtomicU32,
    /// What the last commit left, diffed against by the next; a record
    /// filed before any run (`provisional`) holds predictions only.
    pub(crate) last: Mx<Option<Arc<RunRecord>>>,
    /// Who woke it last and why: the realised chain.
    pub(crate) woke: Mx<Option<Wake>>,
    /// Fields whose walks stopped at it while it had not committed
    /// (`commit::first_hole`), each with the scope depth of the definition
    /// walked from: walked on from it when it commits, is taken or is
    /// removed.
    pub(crate) stopped: Mx<Vec<(Field, u16)>>,
    /// Range readers whose range starts or ends at it: asked again when
    /// it is removed (their bound gone, they answer none).
    pub(crate) bounded: Mx<Vec<(Field, RangeKey, RangeReader)>>,
    /// Nanoseconds of its last run, its commit and its walk forward, and
    /// when it committed (from the evaluation's start).
    pub(crate) run_ns: AtomicU64,
    pub(crate) commit_ns: AtomicU64,
    pub(crate) reask_ns: AtomicU64,
    pub(crate) done_ns: AtomicU64,
    /// Reruns caused by `At::Final` re-asks in this evaluation
    /// (`Limits::final_reruns`), and the build they count in (`Engine::final_gen`); past
    /// the limit the node is capped.
    pub(crate) final_reruns: AtomicU32,
    pub(crate) final_epoch: AtomicU64,
    /// The node whose claim took it (raw id + 1; 0: none), set and
    /// cleared under its segment's write lock (`commit::apply_claim`): a
    /// claim passes over a sibling another claimer holds, and gives back
    /// only its own.
    pub(crate) claimer: AtomicU64,
    /// The furthest sibling its claim holds (raw id + 1; 0: none): a
    /// claim made smaller gives back what it holds up to there, past
    /// siblings it never held.
    pub(crate) claim_end: AtomicU64,
    /// Placed as its placer's continuation (`Ctx::follow`,
    /// `Ctx::follow_after`), not as its child: a node placed after the
    /// placer and its children goes before it.
    pub(crate) follows: AtomicBool,
    /// The node it was placed right after (raw id + 1; 0: pushed by the
    /// client, after none): the `at` of a `Ctx::follow_after`, else the
    /// placer or the placement before it. A placement kept where it stood
    /// is checked against it (`commit::stranded`).
    pub(crate) anchor: AtomicU64,
    /// Its parked state (`Ctx::park`) rests on a read that moved: a run
    /// that went on from it read one kept from the run before that no
    /// longer holds. The next run is offered none (`Ctx::resumed`).
    pub(crate) state_stale: AtomicBool,
    /// Where it stands among the nodes placed at its anchor: 0 a child
    /// of it, 1 after it and its children (another's `Ctx::follow_after`,
    /// or the placement after it in its placer's run), 2 its own
    /// continuation (`Ctx::follow`). Those of a lower slot stand before
    /// those of a higher one.
    pub(crate) slot: AtomicU8,
    /// Its operation's layer (`Operation::layer`), asked when it is made
    /// or its text replaced; and the layer it was queued in: the count of
    /// outstanding nodes it is held in (`sched::Shared::by_layer`) and
    /// the gate at its run's start read `qlayer`, never the operation
    /// again, so a text replaced while it waits keeps the count matched.
    pub(crate) layer: AtomicU8,
    pub(crate) qlayer: AtomicU8,
    /// Its operation's static effect (`Operation::effect`), packed
    /// (`pack_effect`): asked once when the node is made or its text
    /// replaced.
    pub(crate) effect: AtomicU32,
    /// It has brackets filed in its segment.
    pub(crate) has_brackets: AtomicBool,
    /// The fields other nodes' reads anchored at it read (`At::Before`,
    /// `At::AfterOwn`): its removal looks for those readers in its
    /// segment, under those fields alone.
    pub(crate) anchored: Mx<Vec<Field>>,
}

/// An effect as 32 bits: closes, opens, `before`, and whether pending.
pub(crate) fn pack_effect(e: Effect) -> u32 {
    let b = e.brackets();
    u32::from(b.closes) | (u32::from(b.opens) << 8) | (u32::from(b.before) << 16) | (u32::from(e == Effect::Pending) << 17)
}

/// The brackets of a packed effect, and whether it is pending.
pub(crate) fn unpack_effect(x: u32) -> (Brackets, bool) {
    let b = Brackets { closes: (x & 0xff) as u8, opens: ((x >> 8) & 0xff) as u8, before: x & (1 << 16) != 0 };
    (b, x & (1 << 17) != 0)
}

impl Node {
    pub(crate) fn new(
        id: NodeId,
        unit: UnitId,
        op: OpId,
        args: Vec<ValueId>,
        parent: Option<NodeId>,
        seg: SegId,
        pos: Pos,
        layer: u8,
    ) -> Node {
        Node {
            layer: AtomicU8::new(layer),
            qlayer: AtomicU8::new(layer),
            final_reruns: AtomicU32::new(0),
            final_epoch: AtomicU64::new(0),
            claimer: AtomicU64::new(0),
            claim_end: AtomicU64::new(0),
            follows: AtomicBool::new(false),
            anchor: AtomicU64::new(0),
            state_stale: AtomicBool::new(false),
            slot: AtomicU8::new(0),
            effect: AtomicU32::new(0),
            has_brackets: AtomicBool::new(false),
            anchored: Mx::new(Vec::new()),
            id,
            unit,
            op: AtomicU32::new(op.0),
            text: Lock::new(args.into()),
            parent,
            seg: AtomicU32::new(seg.0),
            pos: AtomicU64::new(pos.0),
            state: AtomicU8::new(State::Dirty as u8),
            committing: AtomicBool::new(false),
            runs: AtomicU32::new(0),
            run_epoch: AtomicU64::new(0),
            serial: AtomicU32::new(0),
            last: Mx::new(None),
            woke: Mx::new(None),
            stopped: Mx::new(Vec::new()),
            bounded: Mx::new(Vec::new()),
            run_ns: AtomicU64::new(0),
            commit_ns: AtomicU64::new(0),
            reask_ns: AtomicU64::new(0),
            done_ns: AtomicU64::new(0),
        }
    }

    pub(crate) fn op(&self) -> OpId {
        OpId(self.op.load(Ordering::Acquire))
    }

    pub(crate) fn layer(&self) -> u8 {
        self.layer.load(Ordering::Acquire)
    }

    pub(crate) fn qlayer(&self) -> u8 {
        self.qlayer.load(Ordering::Acquire)
    }

    pub(crate) fn args(&self) -> Arc<[ValueId]> {
        self.text.read().clone()
    }

    pub(crate) fn state(&self) -> State {
        match self.state.load(Ordering::Acquire) {
            0 => State::Clean,
            1 => State::Dirty,
            2 => State::Running,
            3 => State::RunningDirty,
            4 => State::Inert,
            6 => State::RunningInert,
            _ => State::Gone,
        }
    }

    pub(crate) fn cas_state(&self, from: State, to: State) -> bool {
        self.state
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(crate) fn set_state(&self, s: State) {
        self.state.store(s as u8, Ordering::Release);
    }

    /// The node whose claim holds it.
    pub(crate) fn claimer(&self) -> Option<NodeId> {
        self.claimer.load(Ordering::Acquire).checked_sub(1).map(NodeId::from_raw)
    }

    /// Whether a claim of a node before `me` (at `at`) holds it: one
    /// `me` cannot take. A claimer after `me` holds it inside its own
    /// span, which `me` takes with the claimer.
    pub(crate) fn held_before(&self, eng: &crate::engine::Engine, me: NodeId, at: crate::order::Pos) -> bool {
        self.claimer().is_some_and(|c| {
            let cn = eng.node(c);
            c != me && cn.state() != State::Gone && cn.pos_hint() < at
        })
    }

    /// Taken by a claim (`Inert`, or `RunningInert` while a run is in
    /// progress).
    pub(crate) fn taken(&self) -> bool {
        matches!(self.state(), State::Inert | State::RunningInert)
    }

    /// Takes it for a claim: `Inert`, or `RunningInert` while a run of it is
    /// in progress (that run's worker holds its count of the outstanding
    /// nodes). False if it was taken or gone already.
    pub(crate) fn take(&self) -> bool {
        loop {
            let (from, to) = match self.state() {
                s @ (State::Clean | State::Dirty) => (s, State::Inert),
                s @ (State::Running | State::RunningDirty) => (s, State::RunningInert),
                State::Inert | State::RunningInert | State::Gone => return false,
            };
            if self.cas_state(from, to) {
                return true;
            }
        }
    }

    /// Gives it back from a claim: true when it is `Dirty` now and the
    /// caller queues it; a run still in progress is made `RunningDirty`
    /// instead, for its worker to queue once it ends.
    pub(crate) fn release(&self) -> bool {
        loop {
            match self.state() {
                State::Inert => {
                    if self.cas_state(State::Inert, State::Dirty) {
                        return true;
                    }
                }
                State::RunningInert => {
                    if self.cas_state(State::RunningInert, State::RunningDirty) {
                        return false;
                    }
                }
                State::Clean | State::Dirty | State::Running | State::RunningDirty | State::Gone => return false,
            }
        }
    }

    pub(crate) fn seg_hint(&self) -> SegId {
        SegId(self.seg.load(Ordering::Acquire))
    }

    /// The node it was placed right after (none: pushed by the client).
    pub(crate) fn anchor(&self) -> Option<NodeId> {
        match self.anchor.load(Ordering::Acquire) {
            0 => None,
            a => Some(NodeId::from_raw(a - 1)),
        }
    }

    pub(crate) fn slot(&self) -> u8 {
        self.slot.load(Ordering::Acquire)
    }

    pub(crate) fn pos_hint(&self) -> Pos {
        Pos(self.pos.load(Ordering::Acquire))
    }

    /// Whether a run of it has committed.
    pub(crate) fn committed(&self) -> bool {
        self.serial.load(Ordering::Acquire) > 0
    }

    /// Never committed and not taken or removed: its contributions are
    /// still to come.
    pub(crate) fn is_hole(&self) -> bool {
        !self.committed() && matches!(self.state(), State::Dirty | State::Running | State::RunningDirty)
    }

    pub(crate) fn record(&self) -> Option<Arc<RunRecord>> {
        self.last.lock().clone()
    }

    /// Its last run's record, not a prediction.
    pub(crate) fn run_record(&self) -> Option<Arc<RunRecord>> {
        self.record().filter(|r| !r.provisional)
    }
}

/// What a run produced, as the commit took it.
pub(crate) struct RunRecord {
    pub(crate) serial: u32,
    pub(crate) output: ValueId,
    pub(crate) defs: Vec<DefRec>,
    pub(crate) contribs: Vec<(Field, ValueId)>,
    pub(crate) reads: Vec<ReadRec>,
    /// Its placements in order (`Ctx::child`, `Ctx::follow`), matched by
    /// (op, args) at the next commit so unchanged ones are kept.
    pub(crate) placed: Vec<NodeId>,
    pub(crate) claim: usize,
    pub(crate) effect: Effect,
    /// Filed before any run: predictions, replaced by the first commit.
    pub(crate) provisional: bool,
    /// What the run stored for its next run (`Ctx::park`): a long loop's
    /// state at the point it stopped, read back by `Ctx::resumed`. Not a
    /// read: no record, no re-ask. A root at a collection.
    pub(crate) state: Option<ValueId>,
}

impl RunRecord {
    pub(crate) fn provisional(defs: Vec<DefRec>) -> RunRecord {
        RunRecord {
            serial: 0,
            output: ValueId::UNIT,
            defs,
            contribs: Vec::new(),
            reads: Vec::new(),
            placed: Vec::new(),
            claim: 0,
            state: None,
            effect: Effect::None,
            provisional: true,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DefRec {
    pub(crate) f: Field,
    pub(crate) loc: Locality,
    /// For a relation (`Ctx::define_rel`): the value computed from the
    /// reaching input at commit time; `rel` is the constant.
    pub(crate) value: ValueId,
    pub(crate) rel: Option<i64>,
}

/// Where a read looks, relative to the reader.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Anchor {
    Me,
    Before(NodeId),
    AfterOwn(NodeId),
    End(UnitId),
    /// The last definition on the line (`At::Final`).
    Final,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum ReadKind {
    Field {
        f: Field,
        at: Anchor,
    },
    /// The environment's value, whatever is defined before.
    Env {
        f: Field,
    },
    /// The fold over contributions to `f` after `from` (None: the unit's
    /// start) and before `to` (None: the reader), or up to and with `to`'s
    /// own (`incl`).
    Fold {
        f: Field,
        from: Option<NodeId>,
        to: Option<NodeId>,
        incl: bool,
    },
    /// The first contribution to `f` after `after` (None: the unit's start).
    Next {
        f: Field,
        after: Option<NodeId>,
    },
    /// The output of the reader's `i`-th placement; with `guess`, one
    /// that has not run yet answers what the reader saw (its guess,
    /// `Ctx::placed_output_or`).
    Placed {
        i: usize,
        guess: bool,
    },
    /// The `i`-th claimed sibling's (op, args).
    Following {
        i: usize,
    },
}

/// One read: validated by recomputing its answer and comparing to `seen`.
#[derive(Clone, Debug)]
pub(crate) struct ReadRec {
    pub(crate) kind: ReadKind,
    pub(crate) path: PathId,
    pub(crate) seen: Option<ValueId>,
    /// Its field's change count before it read ([`crate::Engine`]'s
    /// `field_epoch`; `u64::MAX` for a read of none).
    pub(crate) epoch: u64,
}

impl ReadKind {
    /// The field it reads, if one.
    pub(crate) fn field(&self) -> Option<Field> {
        match self {
            ReadKind::Field { f, .. } | ReadKind::Env { f } | ReadKind::Fold { f, .. } | ReadKind::Next { f, .. } => Some(*f),
            ReadKind::Placed { .. } | ReadKind::Following { .. } => None,
        }
    }
}

pub(crate) struct Wake {
    pub(crate) by: Option<NodeId>,
    pub(crate) cause: Cause,
    pub(crate) at_ns: u64,
}

/// A placement a run asks for.
pub(crate) enum Placement {
    /// After the placer, before its continuation and next sibling.
    Child(OpId, Vec<ValueId>),
    /// After the placer and its children.
    Follow(OpId, Vec<ValueId>),
    /// After node `at` (and its placements).
    FollowAfter(NodeId, OpId, Vec<ValueId>),
    /// After the node the placer's `j`-th placement of this run stands
    /// for (and its placements), `j` before this one.
    FollowAfterPlaced(usize, OpId, Vec<ValueId>),
}

impl Placement {
    pub(crate) fn key(&self) -> (OpId, &[ValueId]) {
        match self {
            Placement::Child(op, a)
            | Placement::Follow(op, a)
            | Placement::FollowAfter(_, op, a)
            | Placement::FollowAfterPlaced(_, op, a) => (*op, a),
        }
    }
}

/// A run's product, handed to the commit.
pub(crate) struct Product {
    pub(crate) output: ValueId,
    pub(crate) defs: Vec<DefRec>,
    pub(crate) contribs: Vec<(Field, ValueId)>,
    pub(crate) reads: Vec<ReadRec>,
    pub(crate) placements: Vec<Placement>,
    pub(crate) claim: usize,
    /// What the run set (`Ctx::set_effect`); None: its operation's.
    pub(crate) effect: Option<Effect>,
    /// The scope epoch when the run began (`Engine::scope_gen`): a
    /// bracket changed since may have moved any of its reads.
    pub(crate) scope_gen: u64,
    /// The run published nothing new (`Ctx::unchanged`): the last
    /// record's definitions, contributions, placements and output stand,
    /// with this run's reads.
    pub(crate) unchanged: bool,
    /// `Ctx::park`: the state the next run resumes from.
    pub(crate) state: Option<ValueId>,
    /// The run took its last run's state (`Ctx::resumed`): that run's
    /// reads stand with this one's.
    pub(crate) resumed: std::cell::Cell<bool>,
    /// The site of the range the run was woken by (`Ctx::woken_by`,
    /// `Cause::Range`): a kept read from it that still moved at the
    /// commit is one the run did not read again where it was told to go
    /// back before it.
    pub(crate) woken_at: Option<NodeId>,
}
