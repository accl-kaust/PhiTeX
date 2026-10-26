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
pub(crate) const SEG_CAP: usize = 64;
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

    /// Registers segments in one insert (`Engine::push_many`'s).
    pub(crate) fn insert_many(&self, new: Vec<(Pos, SegId)>) {
        let mut segs = self.segs.write();
        segs.extend(new);
        segs.sort_unstable();
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

impl Order {
    /// Gives a segment whose labels are used up (a gap of nothing left
    /// after a relabel: one node placing thousands of placements after
    /// itself) room, from a neighbour of its unit: the segment and its
    /// right neighbour are locked in ascending order, nodes at the end of
    /// the fuller one move into the other so that each holds nodes in
    /// proportion to its labels (`Segment::give`), both relabelled; a
    /// neighbour itself too full is given room from its own neighbour
    /// first. No segment's range changes: the order's list does not, and
    /// a moved node's new segment and field-index entries are filed as a
    /// split files them; the range readers whose interval ended (or began)
    /// at the boundary are widened over the neighbour (`RangeIndex::widen`).
    /// The left neighbour where there is no right one. The one place two
    /// segments are held at once (`lock.rs`'s table). False when the
    /// whole unit is full.
    pub(crate) fn rebalance(&self, eng: &crate::engine::Engine, seg: SegId) -> bool {
        self.rebalance_toward(eng, seg, true) || self.rebalance_toward(eng, seg, false)
    }

    /// `rebalance` with the neighbour on one side only (and its own
    /// neighbours further that way).
    fn rebalance_toward(&self, eng: &crate::engine::Engine, seg: SegId, right: bool) -> bool {
        let s = eng.seg(seg);
        for _ in 0..64 {
            let nb = if right { self.next(seg, s.lo) } else { self.prev(seg, s.lo) };
            let Some(o) = nb.map(|x| eng.seg(x)).filter(|o| o.unit == s.unit) else {
                return false;
            };
            // (ascending: the left one first)
            let (a, b) = if right { (s, o) } else { (o, s) };
            let mut ai = a.inner.write();
            let mut bi = b.inner.write();
            if ai.hi != b.lo {
                // (a split between them meanwhile)
                continue;
            }
            let (si, oi) = if right { (&*ai, &*bi) } else { (&*bi, &*ai) };
            let (ws, wo) = (si.hi.0 - s.lo.0, oi.hi.0 - o.lo.0);
            let (ns, no) = (si.nodes.len() as u64, oi.nodes.len() as u64);
            let total = ns + no + 4;
            if ws.saturating_add(wo) < total.saturating_mul(16) {
                // (the neighbour too full to share: room from its far side
                // first)
                drop(bi);
                drop(ai);
                if !self.rebalance_toward(eng, o.id, right) {
                    return false;
                }
                continue;
            }
            // nodes kept in proportion to labels: s keeps `keep`
            let keep =
                usize::try_from((u128::from(total) * u128::from(ws) / u128::from(ws + wo)) as u64).unwrap_or(0).saturating_sub(2).min(si.nodes.len());
            let give = si.nodes.len() - keep;
            if give == 0 {
                return true;
            }
            let cut = if right {
                *si.nodes.keys().nth(keep).expect("a node to give")
            } else {
                // (the first `give` go left: the cut is the first kept)
                si.nodes.keys().nth(give).copied().unwrap_or(si.hi)
            };
            eng.layout_gen.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            let ((added, gone, brackets), boundary) =
                if right { (s.give(&mut ai, eng, o, &mut bi, cut, true), o.lo) } else { (s.give(&mut bi, eng, o, &mut ai, cut, false), s.lo) };
            let (olo, oid) = (o.lo, o.id);
            for &(f, k) in &added {
                match k {
                    0 => eng.defs.add(f, olo, oid),
                    1 => eng.contribs.add(f, olo, oid),
                    _ => eng.readers.add(f, olo, oid),
                }
            }
            drop(bi);
            drop(ai);
            for &(f, k) in &gone {
                match k {
                    0 => eng.defs.remove(f, s.lo, s.id),
                    1 => eng.contribs.remove(f, s.lo, s.id),
                    _ => eng.readers.remove(f, s.lo, s.id),
                }
            }
            for &(f, _) in added.iter().chain(&gone) {
                eng.bump(f);
            }
            if brackets {
                eng.scope_gen.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                eng.bump(eng.depth_field);
            }
            // (a range read through a moved node now reaches into the
            // neighbour)
            let far = if right { o.inner.read().hi } else { o.lo };
            eng.ranges.widen(boundary, far);
            return true;
        }
        false
    }
}
