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

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct SegId(pub u32);

/// Nodes a segment holds before it splits.
pub(crate) const SEG_CAP: usize = 256;
/// Labels between nodes pushed in a row.
pub(crate) const GAP: u64 = 1 << 20;
/// Labels a unit is given at its making; a unit's segments split within it.
pub(crate) const UNIT_SPAN: u64 = 1 << 44;

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
        self.at_or_after(p).expect("a position before the first segment")
    }

    pub(crate) fn next(&self, _s: SegId, lo: Pos) -> Option<SegId> {
        let segs = self.segs.read();
        let i = segs.partition_point(|(l, _)| *l <= lo);
        segs.get(i).map(|x| x.1)
    }

    pub(crate) fn prev(&self, _s: SegId, lo: Pos) -> Option<SegId> {
        let segs = self.segs.read();
        let i = segs.partition_point(|(l, _)| *l < lo);
        i.checked_sub(1).map(|j| segs[j].1)
    }

    /// Registers a segment starting at `lo` (a split's upper half, a unit's
    /// first segment).
    pub(crate) fn insert(&self, lo: Pos, s: SegId) {
        let mut segs = self.segs.write();
        let i = segs.partition_point(|(l, _)| *l < lo);
        segs.insert(i, (lo, s));
    }

    /// The first segment at or after `p`'s, for walks forward.
    pub(crate) fn at_or_after(&self, p: Pos) -> Option<SegId> {
        let segs = self.segs.read();
        let i = segs.partition_point(|(l, _)| *l <= p);
        i.checked_sub(1).map(|j| segs[j].1)
    }

    /// The segments in order, a snapshot.
    pub(crate) fn all(&self) -> Vec<SegId> {
        self.segs.read().iter().map(|x| x.1).collect()
    }

    pub(crate) fn first(&self) -> Option<SegId> {
        self.segs.read().first().map(|x| x.1)
    }

    pub(crate) fn count(&self) -> u32 {
        u32::try_from(self.segs.read().len()).unwrap_or(u32::MAX)
    }
}
