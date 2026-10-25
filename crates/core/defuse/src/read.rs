//! Answering reads: the reaching definition, a fold, a path.

use crate::api::{NodeId, PathId};
use crate::engine::Engine;
use crate::node::{Anchor, Node, ReadKind};
use crate::order::Pos;
use crate::values::{Field, ValueId};

/// What reaches a position.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Reaching {
    /// None: no definition and no environment value.
    pub(crate) value: Option<ValueId>,
    /// The defining node; None for the environment.
    pub(crate) by: Option<NodeId>,
    pub(crate) provisional: bool,
}

/// The definition of `f` reaching `at` (at `at` too when `inclusive`):
/// `at`'s segment first, then the last earlier segment the field index
/// names, then the environment. Two short read locks at most.
pub(crate) fn reaching(eng: &Engine, f: Field, at: Pos, inclusive: bool) -> Reaching {
    todo!()
}

/// The declared monoid's fold over the contributions to `f` after `from`
/// (None: the start) and before `to` (None: the end): partial sums at the
/// two ends, the segment sums between, combined in order.
pub(crate) fn fold(eng: &Engine, f: Field, from: Option<Pos>, to: Option<Pos>) -> ValueId {
    todo!()
}

/// The first contribution to `f` after `after` (None: the start).
pub(crate) fn next_contribution(eng: &Engine, f: Field, after: Option<Pos>) -> Option<(NodeId, ValueId)> {
    todo!()
}

/// `p` applied to `v` (`PathId::WHOLE`: `v`).
pub(crate) fn apply_path(eng: &Engine, v: Option<ValueId>, p: PathId) -> Option<ValueId> {
    todo!()
}

/// A node's label now, under its segment's read lock; None when gone.
pub(crate) fn pos_of(eng: &Engine, n: NodeId) -> Option<Pos> {
    todo!()
}

/// The position an anchor names for reader `me`, and whether the read is
/// inclusive there; None when the anchor's node is gone (`FieldAt::Gone`).
pub(crate) fn anchor_pos(eng: &Engine, me: &Node, a: Anchor) -> Option<(Pos, bool)> {
    todo!()
}

/// The current answer to a recorded read of `me`, through its path: what a
/// re-ask compares with `seen`. A re-ask that finds the same answer leaves
/// the reader standing; that is the whole of validation.
pub(crate) fn answer(eng: &Engine, me: &Node, kind: &ReadKind, path: PathId) -> Option<ValueId> {
    todo!()
}
