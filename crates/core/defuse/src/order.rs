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

/// Labels a node a rebalanced window keeps at the least.
const DENSITY: u64 = 16;
/// Rebalances a placement asks for before it places where it can.
pub(crate) const ROOM_TRIES: u32 = 2;

impl Order {
    /// Gives a segment whose labels are used up (a gap of nothing left
    /// after a relabel: one node placing thousands of placements after
    /// itself) room: the window of its unit's segments around it, doubled
    /// until it holds `DENSITY` labels a node (the whole unit at the
    /// most, two labels a node then), is relabelled evenly. No segment's
    /// range changes, so neither do the order's list and the field
    /// indexes' keys: each segment is given nodes in proportion to its
    /// labels, a node moving across a boundary to the segment its new
    /// label is in (`Segment::give`, between neighbours, in order), and
    /// filed as a split files it; the range readers whose interval began
    /// or ended inside the window are widened over it (`RangeIndex::
    /// widen_over`). The window's segments are taken in ascending order,
    /// no other lock held (`lock.rs`'s table). False when the whole unit
    /// holds fewer than two labels a node.
    pub(crate) fn rebalance(&self, eng: &crate::engine::Engine, seg: SegId) -> bool {
        let s = eng.seg(seg);
        let (mut left, mut right) = (0usize, 0usize);
        loop {
            // the window's segments, in order, from the list now
            let (ids, at_lo, at_hi) = {
                let segs = self.segs.read();
                let Ok(i) = segs.binary_search(&(s.lo, s.id)) else {
                    return false;
                };
                let mut a = i;
                while a > 0 && i - a < left && eng.seg(segs[a - 1].1).unit == s.unit {
                    a -= 1;
                }
                let mut b = i;
                while b + 1 < segs.len() && b - i < right && eng.seg(segs[b + 1].1).unit == s.unit {
                    b += 1;
                }
                let at_lo = a == 0 || eng.seg(segs[a - 1].1).unit != s.unit;
                let at_hi = b + 1 == segs.len() || eng.seg(segs[b + 1].1).unit != s.unit;
                (segs[a..=b].iter().map(|x| x.1).collect::<Vec<SegId>>(), at_lo, at_hi)
            };
            let win: Vec<std::sync::Arc<crate::segment::Segment>> = ids.iter().map(|&x| eng.seg(x).clone()).collect();
            let mut gs: Vec<crate::lock::WriteGuard<'_, crate::segment::Inner>> = win.iter().map(|x| x.inner.write()).collect();
            // (a split among them meanwhile: the list again)
            if (1..win.len()).any(|k| gs[k - 1].hi != win[k].lo) {
                continue;
            }
            let lo = win[0].lo;
            let width = gs[gs.len() - 1].hi.0 - lo.0;
            let n: u64 = gs.iter().map(|g| g.nodes.len() as u64).sum();
            let whole = at_lo && at_hi;
            if width < DENSITY.saturating_mul(n + 1) && !(whole && width >= 2 * (n + 1)) {
                drop(gs);
                if whole {
                    return false;
                }
                // twice as many segments, each side as far as the unit goes
                let k = left + right + 1;
                let (mut l, mut r) = (k / 2 + 1, k - k / 2);
                if at_lo {
                    r += l;
                    l = 0;
                }
                if at_hi {
                    l += r;
                    r = 0;
                }
                left += l;
                right += r;
                continue;
            }
            self.spread(eng, &win, &mut gs, n);
            let hi = gs[gs.len() - 1].hi;
            drop(gs);
            // (a range read through a moved node now reaches anywhere in
            // the window)
            eng.ranges.widen_over(lo, hi);
            return true;
        }
    }

    /// `rebalance`'s move, under the window's locks: each segment's count
    /// in proportion to its labels (the remainder to the widest, which
    /// have room), the nodes moved across each boundary in turn (rightward
    /// then leftward, every move between neighbours), each segment
    /// relabelled evenly, the field indexes told what each segment holds
    /// now.
    fn spread(
        &self,
        eng: &crate::engine::Engine,
        win: &[std::sync::Arc<crate::segment::Segment>],
        gs: &mut [crate::lock::WriteGuard<'_, crate::segment::Inner>],
        n: u64,
    ) {
        use crate::values::{Field, FxHashMap};
        let k = win.len();
        let widths: Vec<u64> = (0..k).map(|i| gs[i].hi.0 - win[i].lo.0).collect();
        let total: u64 = widths.iter().sum();
        let share = |i: usize| u128::from(n) * u128::from(widths[i]);
        let mut want: Vec<u64> = (0..k).map(|i| (share(i) / u128::from(total)) as u64).collect();
        let mut left = n - want.iter().sum::<u64>();
        let mut by_rest: Vec<usize> = (0..k).collect();
        by_rest.sort_by_key(|&i| std::cmp::Reverse(share(i) % u128::from(total)));
        while left > 0 {
            let before = left;
            for &i in &by_rest {
                if left > 0 && widths[i] >= 4 * (want[i] + 2) {
                    want[i] += 1;
                    left -= 1;
                }
            }
            if left == before {
                let i = (0..k).max_by_key(|&i| widths[i]).expect("a window");
                want[i] += left;
                left = 0;
            }
        }
        let held = |g: &crate::segment::Inner| -> FxHashMap<(Field, u8), ()> {
            let mut m = FxHashMap::default();
            // (an entry made ready for a commit and not filled holds nothing)
            m.extend(g.defs.iter().filter(|x| !x.1.is_empty()).map(|(f, _)| ((*f, 0u8), ())));
            m.extend(g.contribs.iter().filter(|x| !x.1.is_empty()).map(|(f, _)| ((*f, 1u8), ())));
            m.extend(g.readers.keys().map(|f| ((*f, 2u8), ())));
            m
        };
        let before: Vec<_> = gs.iter().map(|g| held(g)).collect();
        eng.layout_gen.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let mut brackets = false;
        let counts = |gs: &[crate::lock::WriteGuard<'_, crate::segment::Inner>]| -> Vec<u64> { gs.iter().map(|g| g.nodes.len() as u64).collect() };
        // rightward: every prefix down to its share or below
        let (mut have, mut aim) = (0u64, 0u64);
        for i in 0..k.saturating_sub(1) {
            let c = counts(gs);
            have += c[i];
            aim += want[i];
            if have > aim {
                let give = (have - aim) as usize;
                let cut = *gs[i].nodes.keys().nth(c[i] as usize - give).expect("a node to give");
                let (a, b) = gs.split_at_mut(i + 1);
                brackets |= win[i].give(&mut a[i], eng, &win[i + 1], &mut b[0], cut, true);
                have = aim;
            }
        }
        // leftward: every prefix up to its share
        for i in (1..k).rev() {
            let c = counts(gs);
            let have: u64 = c[..i].iter().sum();
            let aim: u64 = want[..i].iter().sum();
            if have < aim {
                let take = (aim - have) as usize;
                let cut = gs[i].nodes.keys().nth(take).copied().unwrap_or(crate::order::Pos(u64::MAX));
                let (a, b) = gs.split_at_mut(i);
                brackets |= win[i].give(&mut b[0], eng, &win[i - 1], &mut a[i - 1], cut, false);
            }
        }
        for i in 0..k {
            win[i].relabel(&mut gs[i], eng);
        }
        let mut bump: FxHashMap<Field, ()> = FxHashMap::default();
        for i in 0..k {
            let now = held(&gs[i]);
            for &(f, kind) in now.keys() {
                bump.insert(f, ());
                if !before[i].contains_key(&(f, kind)) {
                    match kind {
                        0 => eng.defs.add(f, win[i].lo, win[i].id),
                        1 => eng.contribs.add(f, win[i].lo, win[i].id),
                        _ => eng.readers.add(f, win[i].lo, win[i].id),
                    }
                }
            }
            for &(f, kind) in before[i].keys() {
                if !now.contains_key(&(f, kind)) {
                    bump.insert(f, ());
                    match kind {
                        0 => eng.defs.remove(f, win[i].lo, win[i].id),
                        1 => eng.contribs.remove(f, win[i].lo, win[i].id),
                        _ => eng.readers.remove(f, win[i].lo, win[i].id),
                    }
                }
            }
        }
        for (f, ()) in bump {
            eng.bump(f);
        }
        if brackets {
            eng.scope_gen.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            eng.bump(eng.depth_field);
        }
    }
}
