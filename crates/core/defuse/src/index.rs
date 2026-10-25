//! Per field, the segments that hold a definition (or a contribution) of
//! it, in order. A read of `f` at `p` goes straight to the last such segment
//! at or before `p`'s; a commit of `f` at `q` walks forward only to the next
//! such segment. A segment enters the index at its first definition of `f`
//! and leaves it at its last one's removal; both are rare, so the index is
//! read-mostly, sharded by field.

use std::collections::BTreeSet;

use crate::lock::Lock;
use crate::order::{Pos, SegId};
use crate::values::{Field, FxHashMap};

const SHARDS: usize = 64;

pub(crate) struct FieldIndex {
    shards: Box<[Lock<FxHashMap<Field, BTreeSet<(Pos, SegId)>>>]>,
}

impl FieldIndex {
    pub(crate) fn new() -> FieldIndex {
        todo!()
    }

    pub(crate) fn add(&self, f: Field, lo: Pos, s: SegId) {
        todo!()
    }

    pub(crate) fn remove(&self, f: Field, lo: Pos, s: SegId) {
        todo!()
    }

    /// The last segment at or before the one starting at `lo` with `f`.
    pub(crate) fn last_at_or_before(&self, f: Field, lo: Pos) -> Option<SegId> {
        todo!()
    }

    /// The first segment after the one starting at `lo` with `f`.
    pub(crate) fn next_after(&self, f: Field, lo: Pos) -> Option<SegId> {
        todo!()
    }

    /// The segments with `f` whose start is in `[from, to)`, in order.
    pub(crate) fn between(&self, f: Field, from: Pos, to: Pos) -> Vec<SegId> {
        todo!()
    }

    pub(crate) fn has(&self, f: Field) -> bool {
        todo!()
    }
}
