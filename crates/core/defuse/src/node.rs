//! A node and what its last run left.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use crate::api::{Cause, Effect, Locality, NodeId, OpId, PathId, UnitId};
use crate::lock::{Lock, Mx};
use crate::order::{Pos, SegId};
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
    /// (`commit::first_hole`): walked on from it when it commits, is taken
    /// or is removed.
    pub(crate) stopped: Mx<Vec<Field>>,
    /// Nanoseconds of its last run, its commit and its walk forward, and
    /// when it committed (from the evaluation's start).
    pub(crate) run_ns: AtomicU64,
    pub(crate) commit_ns: AtomicU64,
    pub(crate) reask_ns: AtomicU64,
    pub(crate) done_ns: AtomicU64,
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
    ) -> Node {
        Node {
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
            run_ns: AtomicU64::new(0),
            commit_ns: AtomicU64::new(0),
            reask_ns: AtomicU64::new(0),
            done_ns: AtomicU64::new(0),
        }
    }

    pub(crate) fn op(&self) -> OpId {
        OpId(self.op.load(Ordering::Acquire))
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
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Anchor {
    Me,
    Before(NodeId),
    AfterOwn(NodeId),
    End(UnitId),
}

#[derive(Clone, PartialEq, Eq, Debug)]
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
    /// The output of the reader's `i`-th placement.
    Placed {
        i: usize,
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
}

impl Placement {
    pub(crate) fn key(&self) -> (OpId, &[ValueId]) {
        match self {
            Placement::Child(op, a) | Placement::Follow(op, a) | Placement::FollowAfter(_, op, a) => (*op, a),
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
    pub(crate) effect: Effect,
}
