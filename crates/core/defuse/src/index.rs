//! Per field, the segments that hold a definition (or a contribution, or a
//! reader) of it, in order. A read of `f` at `p` goes straight to the last
//! such segment at or before `p`'s; a commit of `f` at `q` walks forward only
//! to the next such segment. A segment enters the index at its first
//! definition of `f` and leaves it at its last one's removal; both are rare,
//! so the index is read-mostly, sharded by field.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::engine::EnvReader;
use crate::lock::Lock;
use crate::read::Reaching;
use crate::order::{Pos, SegId};
use crate::segment::{RangeReader, Reader, push_pruned};
use crate::values::{Field, FxHashMap, ValueId, hash_words};

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
            Segs::Many(_) if from >= to => Vec::new(),
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

    /// Whether segment `s`, starting at `lo`, is filed under `f`.
    pub(crate) fn holds(&self, f: Field, lo: Pos, s: SegId) -> bool {
        self.shards[shard(f)].0.read().get(&f).is_some_and(|x| match x {
            Segs::One(p, q) => (*p, *q) == (lo, s),
            Segs::Many(set) => set.contains(&(lo, s)),
        })
    }
}

/// Per field, its readers that are not anchored in a segment: range
/// readers (folds and `next`s), `At::Final` readers and environment
/// readers, in one shard by field. A range reader is filed once as an
/// interval of labels that holds its range for the reader's life: from
/// the start of the segment its first node was in to the end of the one
/// its last was in (a relabel stays within a segment's range, a split only
/// narrows it). Intervals are kept by their span's power of two, each
/// class in order of its start and again in order of its end, so the
/// readers whose interval holds a label are a few short ranges of a few
/// maps, whichever of the two orders gives the shorter.
pub(crate) struct RangeIndex {
    shards: Box<[RangeShard]>,
    seq: AtomicU64,
}

#[repr(align(64))]
struct RangeShard(Lock<FxHashMap<Field, Ranges>>);

/// One span class: by (start, filing), the end and the reader; or by
/// (end, filing), the start and the reader.
type Class = BTreeMap<(Pos, u64), (Pos, RangeReader)>;
/// Classes by their span's power of two, in order.
type Classes = Vec<(u32, Class)>;

#[derive(Default)]
struct Ranges {
    /// By span class (`span <= 1 << k`), by (start, filing).
    classes: Classes,
    /// By span class, by (end, filing).
    ends: Classes,
    /// Each filing's interval now: a rebalance widens one in place
    /// (`RangeIndex::widen`), its key the same.
    at: FxHashMap<u64, (Pos, Pos)>,
    len: usize,
    /// Filed since the dead were last dropped.
    filed: usize,
    /// `At::Final` readers.
    finals: Vec<Reader>,
    /// What its `At::Final` readers are answered: its last definition
    /// when the line was last quiet (`commit::publish_finals`), or what
    /// the first reader found (`read::final_read`).
    published: Option<Reaching>,
    /// Its last definition may have moved since it was published.
    dirty: bool,
    /// Published anew, its readers of another's definition not re-asked
    /// yet (`commit::publish_finals`).
    pending: bool,
    /// Readers of the environment's value.
    envs: Vec<EnvReader>,
}

/// Where a range reader is filed, to take it out again: its filing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RangeKey {
    seq: u64,
}

fn class_of(lo: Pos, hi: Pos) -> u32 {
    let span = hi.0.saturating_sub(lo.0).max(1);
    64 - (span - 1).leading_zeros()
}
fn class_mut(cs: &mut Classes, class: u32) -> &mut Class {
    let i = match cs.binary_search_by_key(&class, |c| c.0) {
        Ok(i) => i,
        Err(i) => {
            cs.insert(i, (class, BTreeMap::new()));
            i
        }
    };
    &mut cs[i].1
}

fn class_take(cs: &mut Classes, class: u32, key: (Pos, u64)) -> Option<RangeReader> {
    let Ok(i) = cs.binary_search_by_key(&class, |c| c.0) else {
        return None;
    };
    let hit = cs[i].1.remove(&key).map(|x| x.1);
    if cs[i].1.is_empty() {
        cs.remove(i);
    }
    hit
}

impl Ranges {
    fn insert(&mut self, seq: u64, lo: Pos, hi: Pos, r: RangeReader) {
        let class = class_of(lo, hi);
        class_mut(&mut self.ends, class).insert((hi, seq), (lo, r.clone()));
        class_mut(&mut self.classes, class).insert((lo, seq), (hi, r));
        self.at.insert(seq, (lo, hi));
        self.len += 1;
        self.filed += 1;
    }

    /// Takes filing `seq` out: its reader, if it was there.
    fn take(&mut self, seq: u64) -> Option<RangeReader> {
        let (lo, hi) = self.at.remove(&seq)?;
        let class = class_of(lo, hi);
        let r = class_take(&mut self.classes, class, (lo, seq));
        class_take(&mut self.ends, class, (hi, seq));
        self.len -= 1;
        r
    }

    fn is_empty(&self) -> bool {
        self.len == 0 && self.finals.is_empty() && self.envs.is_empty() && self.published.is_none() && !self.dirty && !self.pending
    }

    /// Drops the dead once as many have been filed as the list held.
    fn sweep(&mut self, live: impl Fn(&RangeReader) -> bool) {
        if self.filed < 64 || self.filed < self.len / 2 {
            return;
        }
        self.filed = 0;
        let at = &mut self.at;
        for (_, m) in &mut self.classes {
            m.retain(|k, (_, r)| {
                let keep = live(r);
                if !keep {
                    at.remove(&k.1);
                }
                keep
            });
        }
        for (_, m) in &mut self.ends {
            m.retain(|_, (_, r)| live(r));
        }
        self.classes.retain(|c| !c.1.is_empty());
        self.ends.retain(|c| !c.1.is_empty());
        self.len = self.classes.iter().map(|c| c.1.len()).sum();
    }
}

impl RangeIndex {
    pub(crate) fn new() -> RangeIndex {
        RangeIndex { shards: (0..RANGE_SHARDS).map(|_| RangeShard(Lock::new(FxHashMap::default()))).collect(), seq: AtomicU64::new(0) }
    }

    /// Files readers of `f` over the intervals `[lo, hi)`, one lock for
    /// all; the dead are dropped on the way now and then.
    pub(crate) fn insert(&self, f: Field, rs: Vec<(Pos, Pos, RangeReader)>, live: impl Fn(&RangeReader) -> bool) -> Vec<RangeKey> {
        let seq = self.seq.fetch_add(rs.len() as u64, Ordering::Relaxed);
        let keys: Vec<RangeKey> = (0..rs.len()).map(|i| RangeKey { seq: seq + i as u64 }).collect();
        let mut w = self.shards[range_shard(f)].0.write();
        let m = w.entry(f).or_default();
        m.sweep(live);
        for (k, (lo, hi, r)) in keys.iter().zip(rs) {
            m.insert(k.seq, lo, hi, r);
        }
        keys
    }

    /// Takes a reader out; false if it was not there.
    pub(crate) fn remove(&self, f: Field, k: RangeKey) -> bool {
        let mut w = self.shards[range_shard(f)].0.write();
        let Some(m) = w.get_mut(&f) else {
            return false;
        };
        let hit = m.take(k.seq).is_some();
        if m.is_empty() {
            w.remove(&f);
        }
        hit
    }

    /// Files a reader again over `[lo, hi)`, its key the same (a span
    /// found stale after a rebalance); false if it is not there.
    pub(crate) fn refile(&self, f: Field, k: RangeKey, lo: Pos, hi: Pos) -> bool {
        let mut w = self.shards[range_shard(f)].0.write();
        let Some(m) = w.get_mut(&f) else {
            return false;
        };
        match m.at.get(&k.seq) {
            Some(&x) if x == (lo, hi) => true,
            Some(_) => {
                let r = m.take(k.seq).expect("a filing it holds");
                m.insert(k.seq, lo, hi, r);
                true
            }
            None => false,
        }
    }

    pub(crate) fn has(&self, f: Field, k: RangeKey) -> bool {
        self.shards[range_shard(f)].0.read().get(&f).is_some_and(|m| m.at.contains_key(&k.seq))
    }

    /// Every interval starting inside `(lo, hi)` starts at `lo` instead,
    /// and every one ending inside it ends at `hi`: a rebalance moved
    /// nodes among the segments of `[lo, hi)` (`order::Order::rebalance`).
    pub(crate) fn widen_over(&self, lo: Pos, hi: Pos) {
        let inside = |p: Pos| p > lo && p < hi;
        for sh in &*self.shards {
            let mut w = sh.0.write();
            for m in w.values_mut() {
                let hit: Vec<(u64, Pos, Pos)> =
                    m.at.iter().filter(|(_, (a, b))| inside(*a) || inside(*b)).map(|(s, (a, b))| (*s, *a, *b)).collect();
                for (seq, a, b) in hit {
                    let r = m.take(seq).expect("a filing it holds");
                    m.insert(seq, if inside(a) { lo } else { a }, if inside(b) { hi } else { b }, r);
                }
            }
        }
    }

    /// The readers of `f` whose interval holds `at` (a superset of those
    /// whose range covers it). Each class is searched by start and by end
    /// side by side, and the search that ends first answers: a class of
    /// folds from a unit's start costs the folds that end after `at`, not
    /// every fold that starts before it.
    pub(crate) fn holding(&self, f: Field, at: Pos) -> Vec<(RangeKey, RangeReader)> {
        let r = self.shards[range_shard(f)].0.read();
        let Some(m) = r.get(&f) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for ((class, by_lo), (_, by_hi)) in m.classes.iter().zip(&m.ends) {
            let floor = if *class >= 64 { 0 } else { at.0.saturating_sub(1 << class) };
            let ceil = if *class >= 64 { u64::MAX } else { at.0.saturating_add(1 << class) };
            let mut a = by_lo.range((Pos(floor), 0)..(at, 0));
            let mut b = by_hi.range((Pos(at.0.saturating_add(1)), 0)..=(Pos(ceil), u64::MAX));
            let (mut ra, mut rb) = (Vec::new(), Vec::new());
            loop {
                let Some(((_, seq), (hi, rr))) = a.next() else {
                    out.append(&mut ra);
                    break;
                };
                if *hi > at {
                    ra.push((RangeKey { seq: *seq }, rr.clone()));
                }
                let Some(((_, seq), (lo, rr))) = b.next() else {
                    out.append(&mut rb);
                    break;
                };
                if *lo < at {
                    rb.push((RangeKey { seq: *seq }, rr.clone()));
                }
            }
        }
        out
    }

    /// Files an `At::Final` reader of `f`, the dead dropped when the list
    /// is about to grow.
    /// Every value its readers hold ([`crate::Engine::collect`]).
    pub(crate) fn roots(&self, eng: &crate::Engine, keep: &mut dyn FnMut(ValueId)) {
        let live = |n, s| crate::segment::live(eng, n, s);
        for sh in &*self.shards {
            let m = sh.0.read();
            for (f, rs) in m.iter() {
                keep(f.0);
                for (_, c) in rs.classes.iter().chain(rs.ends.iter()) {
                    for (_, r) in c.values().filter(|(_, r)| live(r.node, r.serial)) {
                        r.seen.iter().for_each(|&(_, x)| x.into_iter().for_each(&mut *keep));
                    }
                }
                for r in rs.finals.iter().filter(|r| live(r.node, r.serial)) {
                    r.seen.into_iter().for_each(&mut *keep);
                }
                if let Some(x) = rs.published.and_then(|r| r.value) {
                    keep(x);
                }
                for r in rs.envs.iter().filter(|r| live(r.node, r.serial)) {
                    r.seen.into_iter().for_each(&mut *keep);
                }
            }
        }
    }

    pub(crate) fn add_final(&self, f: Field, r: Reader, live: impl Fn(&Reader) -> bool) {
        let mut w = self.shards[range_shard(f)].0.write();
        push_pruned(&mut w.entry(f).or_default().finals, r, live);
    }

    /// The value published for `f`'s `At::Final` readers, if any.
    pub(crate) fn published(&self, f: Field) -> Option<Reaching> {
        self.shards[range_shard(f)].0.read().get(&f).and_then(|m| m.published)
    }

    /// Publishes `r` for `f` unless a value is published already (a
    /// first reader), marked to be checked at the next quiet point: the
    /// value published.
    pub(crate) fn publish_first(&self, f: Field, r: Reaching) -> Reaching {
        let mut w = self.shards[range_shard(f)].0.write();
        let m = w.entry(f).or_default();
        if let Some(x) = m.published {
            return x;
        }
        m.published = Some(r);
        m.dirty = true;
        r
    }

    /// Marks `f`'s published value to be checked at the next quiet point
    /// (its last definition landed, changed or went); nothing where none
    /// is published.
    pub(crate) fn mark_dirty(&self, f: Field) {
        let sh = &self.shards[range_shard(f)].0;
        if !sh.read().get(&f).is_some_and(|m| m.published.is_some() && !m.dirty) {
            return;
        }
        if let Some(m) = sh.write().get_mut(&f) {
            m.dirty = true;
        }
    }

    /// Marks `f` to be published at the next quiet point, published
    /// before or not: a read-back node defined it (`Operation::read_back`).
    pub(crate) fn mark_read_back(&self, f: Field) {
        let sh = &self.shards[range_shard(f)].0;
        if sh.read().get(&f).is_some_and(|m| m.dirty) {
            return;
        }
        sh.write().entry(f).or_default().dirty = true;
    }

    /// The fields marked since the last take, each unmarked.
    pub(crate) fn take_dirty(&self) -> Vec<Field> {
        let mut out = Vec::new();
        for sh in &*self.shards {
            if !sh.0.read().values().any(|m| m.dirty) {
                continue;
            }
            for (f, m) in sh.0.write().iter_mut() {
                if m.dirty {
                    m.dirty = false;
                    out.push(*f);
                }
            }
        }
        out
    }

    /// Publishes `r` for `f`, marked pending if that changed what is
    /// published.
    pub(crate) fn republish(&self, f: Field, r: Reaching) {
        let mut w = self.shards[range_shard(f)].0.write();
        let m = w.entry(f).or_default();
        if m.published.is_none_or(|x| (x.value, x.by) != (r.value, r.by)) {
            m.pending = true;
        }
        m.published = Some(r);
    }

    /// The fields marked pending, unmarked when `clear`.
    pub(crate) fn pending(&self, clear: bool) -> Vec<Field> {
        let mut out = Vec::new();
        for sh in &*self.shards {
            if !sh.0.read().values().any(|m| m.pending) {
                continue;
            }
            for (f, m) in sh.0.write().iter_mut() {
                if m.pending {
                    m.pending = !clear;
                    out.push(*f);
                }
            }
        }
        out
    }

    /// The `At::Final` readers of `f` that still stand.
    pub(crate) fn finals(&self, f: Field, live: impl Fn(&Reader) -> bool) -> Vec<Reader> {
        self.shards[range_shard(f)].0.read().get(&f).map(|m| m.finals.iter().filter(|r| live(r)).cloned().collect()).unwrap_or_default()
    }

    /// Files environment readers of `f`, the dead dropped as the list
    /// grows.
    pub(crate) fn add_envs(&self, f: Field, rs: Vec<EnvReader>, live: impl Fn(&EnvReader) -> bool) {
        let mut w = self.shards[range_shard(f)].0.write();
        let v = &mut w.entry(f).or_default().envs;
        for r in rs {
            push_pruned(v, r, &live);
        }
    }

    /// The environment readers of `f`, taken out.
    pub(crate) fn take_envs(&self, f: Field) -> Vec<EnvReader> {
        let mut w = self.shards[range_shard(f)].0.write();
        let Some(m) = w.get_mut(&f) else {
            return Vec::new();
        };
        let out = std::mem::take(&mut m.envs);
        if m.is_empty() {
            w.remove(&f);
        }
        out
    }
}
