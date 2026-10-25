//! A run's commit: into the node's own segment, then the walk forward.
//!
//! 1. Under the node's segment's write lock: diff the product's definitions
//!    and contributions against the node's last record, file the changes,
//!    match placements by (op, args) to keep unchanged children, place new
//!    ones in the gap (relabel or split if closed), record the reads
//!    anchored here, bump the serial, swap the record. Release.
//! 2. Reads anchored in other segments: each under that segment's lock, one
//!    at a time, never nested.
//! 3. For each changed (field, position): the walk forward (`reask`). For
//!    each changed contribution: its range readers.
//! 4. Removed placements: `unfile`, recursively, each under its own segment.
//! 5. A relation defined for the first time by this operation: `learn_shape`.
//!
//! No other segment is written in step 1. No lock is held in step 3 except
//! one segment's read lock at a time, and a short write lock to recompute a
//! relation met on the way.

use std::sync::Arc;

use crate::api::{NodeId, OpId};
use crate::engine::Engine;
use crate::node::{Node, Placement, Product};
use crate::order::{Pos, SegId};
use crate::sched::Worker;
use crate::values::{Field, ValueId};

/// Counts for the report.
#[derive(Default, Clone, Copy)]
pub(crate) struct CommitStats {
    pub(crate) woken: u64,
    pub(crate) relative_resolved: u64,
    pub(crate) predictions_confirmed: u64,
    pub(crate) predictions_refuted: u64,
    pub(crate) shaped: u64,
    pub(crate) commit_ns: u64,
    pub(crate) reask_ns: u64,
}

/// Commits `p` as `node`'s run. Woken readers go to `w`'s deque.
pub(crate) fn commit(eng: &Engine, w: &Worker, node: &Arc<Node>, p: Product) -> CommitStats {
    todo!()
}

/// The walk forward from a changed definition of `f` at `from` in `seg`:
/// readers of `f` anchored after `from`, in `seg` and then in each later
/// segment the field index does not let it skip, up to the next definition
/// of `f`. Each reader whose answer through its path differs from what it
/// saw is marked dirty (`sched::mark_dirty`). A relation met as the next
/// definition is recomputed in place (one short write lock) and, if its
/// value changed, the walk goes on past it: the cascade of a counter chain.
pub(crate) fn reask(eng: &Engine, w: &Worker, f: Field, from: Pos, seg: SegId, by: NodeId, st: &mut CommitStats) {
    todo!()
}

/// Re-asks the readers of folds over `f` whose range covers `at`.
pub(crate) fn reask_ranges(eng: &Engine, w: &Worker, f: Field, at: Pos, by: NodeId, st: &mut CommitStats) {
    todo!()
}

/// The first relation `f := input + c` made by operation `op`: every live
/// node of `op` without a committed definition of `f` is given the same
/// relation as a provisional definition at its position, in order, each
/// computed from the one before. Readers of those positions are re-asked
/// and run on the predicted values; the nodes' own runs then confirm them
/// (same value: nobody re-asked) or replace them.
pub(crate) fn learn_shape(eng: &Engine, w: &Worker, op: OpId, f: Field, c: i64, st: &mut CommitStats) {
    todo!()
}

/// Files a new node's provisional definitions: its operation's
/// `predicted_defs`, and the relations its operation is known to make.
pub(crate) fn file_predicted(eng: &Engine, w: Option<&Worker>, node: &Arc<Node>) {
    todo!()
}

/// Removes a node: its definitions, contributions, reads, by-op entry and,
/// recursively, its placements; readers of what it defined are re-asked.
/// Its state becomes `Gone`.
pub(crate) fn unfile(eng: &Engine, w: &Worker, node: &Arc<Node>) {
    todo!()
}

/// Makes a node for a placement after `after` (a label in the gap, the
/// segment relabelled or split if the gap is closed), registers it in the
/// segment and files its predictions. Under the segment's write lock,
/// which the caller holds.
pub(crate) fn place(
    eng: &Engine,
    w: Option<&Worker>,
    parent: &Arc<Node>,
    after: Pos,
    what: &Placement,
) -> Arc<Node> {
    todo!()
}

/// Matches a run's placements to the last run's by (op, args): kept nodes
/// in order, each `Some(id)`, and `None` where a new one is to be made.
pub(crate) fn match_placements(eng: &Engine, last: &[NodeId], new: &[Placement]) -> Vec<Option<NodeId>> {
    todo!()
}

/// The reaching input of a relation at `at` plus `c`, as a value.
pub(crate) fn rel_value(eng: &Engine, f: Field, at: Pos, c: i64) -> ValueId {
    todo!()
}
