//! Per field, the segments that hold a definition (or a contribution, or a
//! reader) of it, in order. A read of `f` at `p` goes straight to the last
//! such segment at or before `p`'s; a commit of `f` at `q` walks forward only
//! to the next such segment. A segment enters the index at its first
//! definition of `f` and leaves it at its last one's removal; both are rare,
//! so the index is read-mostly, sharded by field.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::lock::Lock;
use crate::order::{Pos, SegId};
use crate::segment::RangeReader;
use crate::values::{Field, FxHashMap, hash_words};

/// Many and a cache line each: a commit's index work meets another's only
/// when their fields share a shard (a holder the OS stops then stops the
/// other: few enough meetings that this is seldom).
const SHARDS: usize = 1 << 14;
/// Fields read by ranges are few.
const RANGE_SHARDS: usize = 1 << 12;

#[repr(align(64))]
struct Shard(Lock<FxHashMap<Field, Segs>>);

/// The segments of one field: most fields live in one segment, and that
/// one costs no allocation under the shard's lock.
enum Segs {
    One(Pos, SegId),
    Many(BTreeSet<(Pos, SegId)>),
}

impl Segs {
    fn insert(&mut self, lo: Pos, s: SegId) {
        match self {
            Segs::One(p, q) if (*p, *q) == (lo, s) => {}
            Segs::One(p, q) => {
                let mut set = BTreeSet::new();
                set.insert((*p, *q));
                set.insert((lo, s));
                *self = Segs::Many(set);
            }
            Segs::Many(set) => {
                set.insert((lo, s));
            }
        }
    }

    /// Whether it is empty after the removal.
    fn remove(&mut self, lo: Pos, s: SegId) -> bool {
        match self {
            Segs::One(p, q) => (*p, *q) == (lo, s),
            Segs::Many(set) => {
                set.remove(&(lo, s));
                set.is_empty()
            }
        }
    }

    fn last_before(&self, lo: Pos) -> Option<SegId> {
        match self {
            Segs::One(p, q) => (*p < lo).then_some(*q),
            Segs::Many(set) => set.range(..(lo, SegId(0))).next_back().map(|x| x.1),
        }
    }

    fn first_at_or_after(&self, lo: Pos) -> Option<(Pos, SegId)> {
        match self {
            Segs::One(p, q) => (*p >= lo).then_some((*p, *q)),
            Segs::Many(set) => set.range((lo, SegId(0))..).next().copied(),
        }
    }

    fn between(&self, from: Pos, to: Pos) -> Vec<SegId> {
        match self {
            Segs::One(p, q) => (*p >= from && *p < to).then_some(*q).into_iter().collect(),
            Segs::Many(set) => set.range((from, SegId(0))..(to, SegId(0))).map(|x| x.1).collect(),
        }
    }
}

pub(crate) struct FieldIndex {
    shards: Box<[Shard]>,
}

fn shard(f: Field) -> usize {
    // (mixed again: fields are numbered in a row, and one round of the
    // word hash left half the shards unused)
    let h = hash_words(7, &[f.0.raw()]);
    usize::try_from((h ^ (h >> 29)).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 50).unwrap_or(0) & (SHARDS - 1)
}

fn range_shard(f: Field) -> usize {
    shard(f) & (RANGE_SHARDS - 1)
}

impl FieldIndex {
    pub(crate) fn new() -> FieldIndex {
        FieldIndex {
            shards: (0..SHARDS).map(|_| Shard(Lock::new(FxHashMap::default()))).collect(),
        }
    }

    pub(crate) fn add(&self, f: Field, lo: Pos, s: SegId) {
        let mut w = self.shards[shard(f)].0.write();
        match w.get_mut(&f) {
            Some(segs) => segs.insert(lo, s),
            None => {
                w.insert(f, Segs::One(lo, s));
            }
        }
    }

    pub(crate) fn remove(&self, f: Field, lo: Pos, s: SegId) {
        let mut w = self.shards[shard(f)].0.write();
        if let Some(segs) = w.get_mut(&f)
            && segs.remove(lo, s)
        {
            w.remove(&f);
        }
    }

    /// The last segment strictly before the one starting at `lo` with `f`.
    pub(crate) fn last_before(&self, f: Field, lo: Pos) -> Option<SegId> {
        self.shards[shard(f)].0.read().get(&f)?.last_before(lo)
    }

    /// The first segment at or after the one starting at `lo` with `f`, and
    /// its start.
    pub(crate) fn first_at_or_after(&self, f: Field, lo: Pos) -> Option<(Pos, SegId)> {
        self.shards[shard(f)].0.read().get(&f)?.first_at_or_after(lo)
    }

    /// The segments with `f` whose start is in `[from, to)`, in order.
    pub(crate) fn between(&self, f: Field, from: Pos, to: Pos) -> Vec<SegId> {
        self.shards[shard(f)].0.read().get(&f).map(|s| s.between(from, to)).unwrap_or_default()
    }

    pub(crate) fn has(&self, f: Field) -> bool {
        self.shards[shard(f)].0.read().contains_key(&f)
    }
}

/// Per field, its range readers (folds and `next`s), each filed once as an
/// interval of labels that holds its range for the reader's life: from the
/// start of the segment its first node was in to the end of the one its
/// last was in (a relabel stays within a segment's range, a split only
/// narrows it). Intervals are kept by their span's power of two, each
/// class in order of its start, so the readers whose interval holds a label
/// are a few short ranges of a few maps.
pub(crate) struct RangeIndex {
    shards: Box<[RangeShard]>,
    seq: AtomicU64,
}

#[repr(align(64))]
struct RangeShard(Lock<FxHashMap<Field, Ranges>>);

/// One span class: by (start, filing), the end and the reader.
type Class = BTreeMap<(Pos, u64), (Pos, RangeReader)>;

#[derive(Default)]
struct Ranges {
    /// By span class (`span <= 1 << k`), by (start, filing).
    classes: Vec<(u32, Class)>,
    len: usize,
    /// Filed since the dead were last dropped.
    filed: usize,
}

/// Where a range reader is filed, to take it out again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RangeKey {
    class: u32,
    lo: Pos,
    seq: u64,
}

fn class_of(lo: Pos, hi: Pos) -> u32 {
    let span = hi.0.saturating_sub(lo.0).max(1);
    64 - (span - 1).leading_zeros()
}

impl Ranges {
    fn insert(&mut self, k: RangeKey, hi: Pos, r: RangeReader) {
        let i = match self.classes.binary_search_by_key(&k.class, |c| c.0) {
            Ok(i) => i,
            Err(i) => {
                self.classes.insert(i, (k.class, BTreeMap::new()));
                i
            }
        };
        self.classes[i].1.insert((k.lo, k.seq), (hi, r));
        self.len += 1;
        self.filed += 1;
    }

    fn remove(&mut self, k: RangeKey) -> bool {
        let Ok(i) = self.classes.binary_search_by_key(&k.class, |c| c.0) else {
            return false;
        };
        let hit = self.classes[i].1.remove(&(k.lo, k.seq)).is_some();
        if hit {
            self.len -= 1;
            if self.classes[i].1.is_empty() {
                self.classes.remove(i);
            }
        }
        hit
    }

    /// Drops the dead once as many have been filed as the list held.
    fn sweep(&mut self, live: impl Fn(&RangeReader) -> bool) {
        if self.filed < 64 || self.filed < self.len / 2 {
            return;
        }
        self.filed = 0;
        for (_, m) in &mut self.classes {
            m.retain(|_, (_, r)| live(r));
        }
        self.classes.retain(|c| !c.1.is_empty());
        self.len = self.classes.iter().map(|c| c.1.len()).sum();
    }
}

impl RangeIndex {
    pub(crate) fn new() -> RangeIndex {
        RangeIndex {
            shards: (0..RANGE_SHARDS).map(|_| RangeShard(Lock::new(FxHashMap::default()))).collect(),
            seq: AtomicU64::new(0),
        }
    }

    /// Files readers of `f` over the intervals `[lo, hi)`, one lock for
    /// all; the dead are dropped on the way now and then.
    pub(crate) fn insert(
        &self,
        f: Field,
        rs: Vec<(Pos, Pos, RangeReader)>,
        live: impl Fn(&RangeReader) -> bool,
    ) -> Vec<RangeKey> {
        let seq = self.seq.fetch_add(rs.len() as u64, Ordering::Relaxed);
        let keys: Vec<RangeKey> = rs
            .iter()
            .enumerate()
            .map(|(i, (lo, hi, _))| RangeKey {
                class: class_of(*lo, *hi),
                lo: *lo,
                seq: seq + i as u64,
            })
            .collect();
        let mut w = self.shards[range_shard(f)].0.write();
        let m = w.entry(f).or_default();
        m.sweep(live);
        for (k, (_, hi, r)) in keys.iter().zip(rs) {
            m.insert(*k, hi, r);
        }
        keys
    }

    /// Takes a reader out; false if it was not there.
    pub(crate) fn remove(&self, f: Field, k: RangeKey) -> bool {
        let mut w = self.shards[range_shard(f)].0.write();
        let Some(m) = w.get_mut(&f) else {
            return false;
        };
        let hit = m.remove(k);
        if m.len == 0 {
            w.remove(&f);
        }
        hit
    }

    pub(crate) fn has(&self, f: Field, k: RangeKey) -> bool {
        self.shards[range_shard(f)].0.read().get(&f).is_some_and(|m| {
            m.classes
                .binary_search_by_key(&k.class, |c| c.0)
                .is_ok_and(|i| m.classes[i].1.contains_key(&(k.lo, k.seq)))
        })
    }

    /// The readers of `f` whose interval holds `at` (a superset of those
    /// whose range covers it).
    pub(crate) fn holding(&self, f: Field, at: Pos) -> Vec<(RangeKey, RangeReader)> {
        let r = self.shards[range_shard(f)].0.read();
        let Some(m) = r.get(&f) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (class, map) in &m.classes {
            let floor = if *class >= 64 { 0 } else { at.0.saturating_sub(1 << class) };
            for ((lo, seq), (hi, rr)) in map.range((Pos(floor), 0)..(at, 0)) {
                if *hi > at {
                    out.push((
                        RangeKey {
                            class: *class,
                            lo: *lo,
                            seq: *seq,
                        },
                        rr.clone(),
                    ));
                }
            }
        }
        out
    }
}
