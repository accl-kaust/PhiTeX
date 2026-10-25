//! A node and what its last run left.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::api::{Cause, Effect, Locality, NodeId, OpId, PathId, UnitId};
use crate::lock::Mx;
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
}

pub(crate) struct Node {
    pub(crate) id: NodeId,
    pub(crate) unit: UnitId,
    pub(crate) op: OpId,
    pub(crate) args: Box<[ValueId]>,
    pub(crate) parent: Option<NodeId>,
    /// Its segment and label, written under the owning segment's write
    /// lock (a relabel, a split), read under its read lock. A lock-free read
    /// (`pos_hint`) is good enough to find the segment to lock.
    pub(crate) seg: AtomicU32,
    pub(crate) pos: AtomicU64,
    pub(crate) state: AtomicU8,
    /// Runs in this evaluation (`Limits::runs_per_node`).
    pub(crate) runs: AtomicU32,
    /// Bumped at each commit; a reader record carrying an older serial is
    /// stale and dropped when met, so a commit never visits other segments
    /// to retract its node's old reads.
    pub(crate) serial: AtomicU32,
    /// What the last commit left, diffed against by the next.
    pub(crate) last: Mx<Option<Arc<RunRecord>>>,
    /// Who woke it last and why: the realised chain.
    pub(crate) woke: Mx<Option<Wake>>,
    /// With `profile`: nanoseconds of its last run and when it committed.
    pub(crate) run_ns: AtomicU64,
    pub(crate) done_ns: AtomicU64,
}

impl Node {
    pub(crate) fn state(&self) -> State {
        todo!()
    }

    pub(crate) fn cas_state(&self, from: State, to: State) -> bool {
        todo!()
    }

    pub(crate) fn set_state(&self, s: State) {
        self.state.store(s as u8, Ordering::Release);
    }

    pub(crate) fn seg_hint(&self) -> SegId {
        SegId(self.seg.load(Ordering::Acquire))
    }

    pub(crate) fn pos_hint(&self) -> Pos {
        Pos(self.pos.load(Ordering::Acquire))
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
}

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
    /// start) and before `to` (None: the reader).
    Fold {
        f: Field,
        from: Option<NodeId>,
        to: Option<NodeId>,
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
