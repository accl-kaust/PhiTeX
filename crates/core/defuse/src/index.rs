//! Per field, the segments that hold a definition (or a contribution, or a
//! reader) of it, in order. A read of `f` at `p` goes straight to the last
//! such segment at or before `p`'s; a commit of `f` at `q` walks forward only
//! to the next such segment. A segment enters the index at its first
//! definition of `f` and leaves it at its last one's removal; both are rare,
//! so the index is read-mostly, sharded by field.

use std::collections::BTreeSet;

use crate::lock::Lock;
use crate::order::{Pos, SegId};
use crate::values::{Field, FxHashMap, hash_words};

const SHARDS: usize = 512;

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
    shards: Box<[Lock<FxHashMap<Field, Segs>>]>,
}

fn shard(f: Field) -> usize {
    usize::try_from(hash_words(7, &[f.0.raw()]) >> 55).unwrap_or(0) & (SHARDS - 1)
}

impl FieldIndex {
    pub(crate) fn new() -> FieldIndex {
        FieldIndex {
            shards: (0..SHARDS).map(|_| Lock::new(FxHashMap::default())).collect(),
        }
    }

    pub(crate) fn add(&self, f: Field, lo: Pos, s: SegId) {
        let mut w = self.shards[shard(f)].write();
        match w.get_mut(&f) {
            Some(segs) => segs.insert(lo, s),
            None => {
                w.insert(f, Segs::One(lo, s));
            }
        }
    }

    pub(crate) fn remove(&self, f: Field, lo: Pos, s: SegId) {
        let mut w = self.shards[shard(f)].write();
        if let Some(segs) = w.get_mut(&f)
            && segs.remove(lo, s)
        {
            w.remove(&f);
        }
    }

    /// The last segment strictly before the one starting at `lo` with `f`.
    pub(crate) fn last_before(&self, f: Field, lo: Pos) -> Option<SegId> {
        self.shards[shard(f)].read().get(&f)?.last_before(lo)
    }

    /// The first segment at or after the one starting at `lo` with `f`, and
    /// its start.
    pub(crate) fn first_at_or_after(&self, f: Field, lo: Pos) -> Option<(Pos, SegId)> {
        self.shards[shard(f)].read().get(&f)?.first_at_or_after(lo)
    }

    /// The segments with `f` whose start is in `[from, to)`, in order.
    pub(crate) fn between(&self, f: Field, from: Pos, to: Pos) -> Vec<SegId> {
        self.shards[shard(f)].read().get(&f).map(|s| s.between(from, to)).unwrap_or_default()
    }

    pub(crate) fn has(&self, f: Field) -> bool {
        self.shards[shard(f)].read().contains_key(&f)
    }
}
