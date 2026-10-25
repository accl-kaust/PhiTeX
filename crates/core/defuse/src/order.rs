//! The line: positions as labels, cut into segments.
//!
//! A position is a 64-bit label. Nodes pushed to a unit are labelled `GAP`
//! apart; a node placed after another (a child, a continuation) gets a label
//! in the gap before the next node. When a gap closes, the owning segment
//! relabels its own nodes evenly within its range, under its own lock: a
//! node's label is read only under that lock, and readers anchor to nodes,
//! never to labels, so nothing outside the segment holds one.
//!
//! A segment is a range `[lo, hi)` of labels holding at most `SEG_CAP` nodes;
//! past that it splits at its median label, the upper half into a new
//! segment (nodes keep their labels). The order's list maps a label to its
//! segment by binary search and changes only on a split or a new unit.

use crate::lock::Lock;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct Pos(pub u64);

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct SegId(pub u32);

/// Nodes a segment holds before it splits.
pub(crate) const SEG_CAP: usize = 256;
/// Labels between nodes pushed in a row.
pub(crate) const GAP: u64 = 1 << 24;
/// Labels a unit is given at its making; a unit's segments split within it.
pub(crate) const UNIT_SPAN: u64 = 1 << 40;

/// The segments in order of their first label.
pub(crate) struct Order {
    pub(crate) segs: Lock<Vec<(Pos, SegId)>>,
}

impl Order {
    pub(crate) fn new() -> Order {
        Order {
            segs: Lock::new(Vec::new()),
        }
    }

    /// The segment whose range holds `p`.
    pub(crate) fn seg_at(&self, p: Pos) -> SegId {
        todo!()
    }

    pub(crate) fn next(&self, s: SegId, lo: Pos) -> Option<SegId> {
        todo!()
    }

    pub(crate) fn prev(&self, s: SegId, lo: Pos) -> Option<SegId> {
        todo!()
    }

    /// Registers a segment starting at `lo` (a split's upper half, a unit's
    /// first segment).
    pub(crate) fn insert(&self, lo: Pos, s: SegId) {
        todo!()
    }

    /// The first segment at or after `p`'s, for walks forward.
    pub(crate) fn at_or_after(&self, p: Pos) -> Option<SegId> {
        todo!()
    }

    pub(crate) fn count(&self) -> u32 {
        todo!()
    }
}
