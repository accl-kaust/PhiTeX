//! A segment: a bounded range of the line and everything about it.
//!
//! The segment is the unit of locking. A read of field `f` at `p` locks
//! `p`'s segment for one map lookup (and, through the field index, the one
//! earlier segment that last defines `f`). A commit locks the committing
//! node's segment for one diff. Nothing ever holds two segments.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::api::{NodeId, OpId, PathId, UnitId};
use crate::engine::Engine;
use crate::lock::Lock;
use crate::order::{Pos, SegId};
use crate::values::{Field, FxHashMap, ValueId};

pub(crate) struct Segment {
    pub(crate) id: SegId,
    pub(crate) unit: UnitId,
    /// Its first label, fixed for its life (a split keeps the lower half here).
    pub(crate) lo: Pos,
    pub(crate) inner: Lock<Inner>,
}

pub(crate) struct Inner {
    /// One past its last label.
    pub(crate) hi: Pos,
    pub(crate) nodes: BTreeMap<Pos, NodeId>,
    /// Definitions by field, in position order. A relation's value is kept
    /// materialised and recomputed in place when its input changes.
    pub(crate) defs: FxHashMap<Field, BTreeMap<Pos, Def>>,
    /// Readers of each field anchored in this segment. Stale records (an
    /// older serial than the reader's) are dropped when met.
    pub(crate) readers: FxHashMap<Field, Vec<Reader>>,
    /// Contributions by field, in position order, and their fold.
    pub(crate) contribs: FxHashMap<Field, BTreeMap<Pos, ValueId>>,
    pub(crate) sums: FxHashMap<Field, ValueId>,
    /// Readers of folds whose range covers this segment.
    pub(crate) range_readers: FxHashMap<Field, Vec<RangeReader>>,
    /// Its nodes by operation, for `commit::learn_shape`.
    pub(crate) by_op: FxHashMap<OpId, Vec<NodeId>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Def {
    pub(crate) value: ValueId,
    pub(crate) by: NodeId,
    /// `Some(c)`: value = the definition reaching `by` + c.
    pub(crate) rel: Option<i64>,
    /// Filed before its node ran (a prediction, a learned shape): the run
    /// replaces it.
    pub(crate) provisional: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Reader {
    pub(crate) node: NodeId,
    pub(crate) serial: u32,
    /// Whose position the read is at (the reader itself, or the node an
    /// `At` names).
    pub(crate) anchor: NodeId,
    /// `At::AfterOwn`: the anchor's own definition counts.
    pub(crate) inclusive: bool,
    pub(crate) path: PathId,
    pub(crate) seen: Option<ValueId>,
}

#[derive(Clone, Debug)]
pub(crate) struct RangeReader {
    pub(crate) node: NodeId,
    pub(crate) serial: u32,
    pub(crate) from: Option<NodeId>,
    pub(crate) to: Option<NodeId>,
    pub(crate) path: PathId,
    pub(crate) seen: Option<ValueId>,
}

impl Segment {
    pub(crate) fn new(id: SegId, unit: UnitId, lo: Pos, hi: Pos) -> Segment {
        todo!()
    }

    /// The last definition of `f` before `p` (at `p` too when `inclusive`).
    pub(crate) fn reaching<'a>(inner: &'a Inner, f: Field, p: Pos, inclusive: bool) -> Option<(Pos, &'a Def)> {
        todo!()
    }

    /// The first definition of `f` after `p`.
    pub(crate) fn next_def<'a>(inner: &'a Inner, f: Field, p: Pos) -> Option<(Pos, &'a Def)> {
        todo!()
    }

    /// A label strictly after `after` and before the next node (or `hi`);
    /// None when that gap is closed and the segment must relabel or split.
    pub(crate) fn gap_after(inner: &Inner, after: Pos) -> Option<Pos> {
        todo!()
    }

    /// Spreads its nodes' labels evenly over `[lo, hi)`, moving each node's
    /// `pos` and re-keying `nodes`, `defs` and `contribs`. Under the write
    /// lock; nothing outside holds a label.
    pub(crate) fn relabel(&self, inner: &mut Inner, eng: &Engine) {
        todo!()
    }

    pub(crate) fn needs_split(inner: &Inner) -> bool {
        todo!()
    }

    /// Moves the nodes at and after the median label into a new segment
    /// (their labels unchanged, their `seg` updated), with their
    /// definitions, readers, contributions, sums, range readers and by-op
    /// entries; `hi` shrinks to the median. The caller registers the new
    /// segment in the order and in the field indexes.
    pub(crate) fn split(&self, inner: &mut Inner, eng: &Engine, new_id: SegId) -> Arc<Segment> {
        todo!()
    }
}
