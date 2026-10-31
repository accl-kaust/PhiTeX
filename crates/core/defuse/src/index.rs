//! Per field, the segments that hold a definition (or a contribution, or a
//! reader) of it, in order. A read of `f` at `p` goes straight to the last
//! such segment at or before `p`'s; a commit of `f` at `q` walks forward only
//! to the next such segment. A segment enters the index at its first
//! definition of `f` and leaves it at its last one's removal; both are rare,
//! so the index is read-mostly, sharded by field.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::api::UnitId;
use crate::engine::EnvReader;
use crate::lock::Lock;
use crate::order::{Pos, SegId};
use crate::read::Reaching;
use crate::segment::{RangeReader, Reader, push_pruned};
use crate::values::{Field, FxHashMap, ValueId, hash_words};

/// Many and a cache line each: a commit's index work meets another's only
/// when their fields share a shard (a holder the OS stops then stops the
/// other: few enough meetings that this is seldom).
const SHARDS: usize = 1 << 14;
/// Fields read by ranges are few.
const RANGE_SHARDS: usize = 1 << 12;

#[repr(align(64))]
struct Shard(Lock<Entries>);

/// A shard's (field, segment start, segment) triples in order: a field's
/// segments are a run of them. Room for `reserved` more is kept: a
/// commit reserves what its filing may add under its segment's lock
/// (`FieldIndex::reserve`), and that adds into room made before it, so
/// nothing is allocated where other threads wait; a removal frees
/// nothing.
#[derive(Default)]
struct Entries {
    v: Vec<(Field, Pos, SegId)>,
    reserved: usize,
}

impl Entries {
    /// The run of `f`'s entries.
    fn of(&self, f: Field) -> &[(Field, Pos, SegId)] {
        let a = self.v.partition_point(|e| e.0 < f);
        let b = a + self.v[a..].partition_point(|e| e.0 == f);
        &self.v[a..b]
    }

    fn free(&self) -> usize {
        self.v.capacity() - self.v.len()
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

/// A commit's room in a field index's shard (`FieldIndex::reserve`):
/// the index (its address), the shard, and how many adds it still covers.
#[derive(Clone, Copy, Debug)]
struct Reserved {
    ix: usize,
    shard: u32,
    left: u32,
}

thread_local! {
    /// The room this thread's commit holds, under its segment's lock
    /// (`FieldIndex::reserve`): kept by the thread, its own room grown
    /// before the lock (`FieldIndex::prepare`).
    static HELD: std::cell::RefCell<Vec<Reserved>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A shard without room for a commit's adds: the room to make
/// (`FieldIndex::make_room`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Room {
    shard: u32,
    cap: usize,
}

/// The room made for a shard, outside every lock (`FieldIndex::make_room`).
pub(crate) struct MadeRoom {
    shard: u32,
    v: Vec<(Field, Pos, SegId)>,
}

impl FieldIndex {
    pub(crate) fn new() -> FieldIndex {
        FieldIndex {
            shards: (0..SHARDS).map(|_| Shard(Lock::new(Entries::default()))).collect(),
        }
    }

    fn addr(&self) -> usize {
        std::ptr::from_ref(self) as usize
    }

    /// Adds `s` to `f`'s segments: into room this thread's commit
    /// reserved in its shard (`reserve`), else growing the shard if it
    /// must (a split, a rebalance, a placement outside a commit's lock),
    /// leaving the room others reserved.
    pub(crate) fn add(&self, f: Field, lo: Pos, s: SegId) {
        let sh = shard(f);
        let me = self.addr();
        HELD.with(|h| {
            let mut h = h.borrow_mut();
            let held = h
                .iter_mut()
                .find(|x| x.ix == me && x.shard as usize == sh && x.left > 0);
            let mut w = self.shards[sh].0.write();
            let Err(i) = w.v.binary_search(&(f, lo, s)) else {
                return;
            };
            match held {
                Some(x) => {
                    x.left -= 1;
                    w.reserved -= 1;
                }
                None => {
                    if w.free() <= w.reserved {
                        let more = w.reserved + 1;
                        w.v.reserve(more);
                    }
                }
            }
            w.v.insert(i, (f, lo, s));
        });
    }

    /// Room in this thread's list of reservations for a commit of `n`
    /// fields, made before its lock (kept by the thread: grown seldom).
    pub(crate) fn prepare(n: usize) {
        HELD.with(|h| {
            let mut h = h.borrow_mut();
            let more = (3 * n).saturating_sub(h.capacity() - h.len());
            h.reserve(more);
        });
    }

    /// Reserves room for one add in each of the shards `fs` (a shard as
    /// many times as adds to it), each under its lock, held by this
    /// thread until `release`d; the shards without room go to `need`,
    /// none reserved in them. Under a segment's lock, as an add is;
    /// allocates nothing where `need` and the thread's list have room
    /// (`prepare`).
    pub(crate) fn reserve(&self, fs: &mut [usize], need: &mut Vec<Room>) {
        fs.sort_unstable();
        let me = self.addr();
        HELD.with(|h| {
            let mut h = h.borrow_mut();
            let mut i = 0;
            while i < fs.len() {
                let sh = fs[i];
                let k = fs[i..].iter().take_while(|&&x| x == sh).count();
                i += k;
                let mut w = self.shards[sh].0.write();
                let shard = u32::try_from(sh).unwrap_or(u32::MAX);
                if w.free() >= w.reserved + k {
                    w.reserved += k;
                    h.push(Reserved {
                        ix: me,
                        shard,
                        left: u32::try_from(k).unwrap_or(u32::MAX),
                    });
                } else {
                    need.push(Room {
                        shard,
                        cap: (w.v.len() + w.reserved + k).max(w.v.capacity() * 2).max(4),
                    });
                }
            }
        });
    }

    /// The room this thread holds in this index and did not add into,
    /// given back.
    pub(crate) fn release(&self) {
        let me = self.addr();
        HELD.with(|h| {
            h.borrow_mut().retain(|r| {
                if r.ix != me {
                    return true;
                }
                if r.left > 0 {
                    self.shards[r.shard as usize].0.write().reserved -= r.left as usize;
                }
                false
            });
        });
    }

    /// The shard of `f`, for `reserve`.
    pub(crate) fn shard_of(f: Field) -> usize {
        shard(f)
    }

    /// The room `n` names, made under no lock.
    pub(crate) fn make_room(n: &Room) -> MadeRoom {
        MadeRoom {
            shard: n.shard,
            v: Vec::with_capacity(n.cap),
        }
    }

    /// Moves the shard's entries into the room made, if it is more; the
    /// entries' old room is left in `m`, to be freed after the lock.
    pub(crate) fn install(&self, m: &mut MadeRoom) {
        let mut w = self.shards[m.shard as usize].0.write();
        if m.v.capacity() > w.v.capacity() && m.v.capacity() >= w.v.len() {
            m.v.extend_from_slice(&w.v);
            std::mem::swap(&mut w.v, &mut m.v);
            m.v.clear();
        }
    }

    pub(crate) fn remove(&self, f: Field, lo: Pos, s: SegId) {
        let mut w = self.shards[shard(f)].0.write();
        if let Ok(i) = w.v.binary_search(&(f, lo, s)) {
            w.v.remove(i);
        }
    }

    /// The last segment strictly before the one starting at `lo` with `f`.
    pub(crate) fn last_before(&self, f: Field, lo: Pos) -> Option<SegId> {
        let r = self.shards[shard(f)].0.read();
        let xs = r.of(f);
        xs[..xs.partition_point(|e| e.1 < lo)].last().map(|e| e.2)
    }

    /// The first segment at or after the one starting at `lo` with `f`, and
    /// its start.
    pub(crate) fn first_at_or_after(&self, f: Field, lo: Pos) -> Option<(Pos, SegId)> {
        let r = self.shards[shard(f)].0.read();
        let xs = r.of(f);
        xs.get(xs.partition_point(|e| e.1 < lo)).map(|e| (e.1, e.2))
    }

    /// The segments with `f` whose start is in `[from, to)`, in order.
    pub(crate) fn between(&self, f: Field, from: Pos, to: Pos) -> Vec<SegId> {
        if from >= to {
            return Vec::new();
        }
        let r = self.shards[shard(f)].0.read();
        let xs = r.of(f);
        let a = xs.partition_point(|e| e.1 < from);
        let b = xs.partition_point(|e| e.1 < to);
        xs[a..b].iter().map(|e| e.2).collect()
    }

    pub(crate) fn has(&self, f: Field) -> bool {
        !self.shards[shard(f)].0.read().of(f).is_empty()
    }

    /// Whether segment `s`, starting at `lo`, is filed under `f`.
    pub(crate) fn holds(&self, f: Field, lo: Pos, s: SegId) -> bool {
        self.shards[shard(f)].0.read().v.binary_search(&(f, lo, s)).is_ok()
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
    /// `At::End` readers, with the unit whose end they read at: filed
    /// here, not at a node, and re-asked by every walk of the field that
    /// passes that unit's end (`commit::reask_ends`).
    ends_of: Vec<(UnitId, Reader)>,
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
        self.len == 0
            && self.finals.is_empty()
            && self.envs.is_empty()
            && self.ends_of.is_empty()
            && self.published.is_none()
            && !self.dirty
            && !self.pending
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
        RangeIndex {
            shards: (0..RANGE_SHARDS)
                .map(|_| RangeShard(Lock::new(FxHashMap::default())))
                .collect(),
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
        self.shards[range_shard(f)]
            .0
            .read()
            .get(&f)
            .is_some_and(|m| m.at.contains_key(&k.seq))
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
                    m.at.iter()
                        .filter(|(_, (a, b))| inside(*a) || inside(*b))
                        .map(|(s, (a, b))| (*s, *a, *b))
                        .collect();
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
            let floor = if *class >= 64 {
                0
            } else {
                at.0.saturating_sub(1 << class)
            };
            let ceil = if *class >= 64 {
                u64::MAX
            } else {
                at.0.saturating_add(1 << class)
            };
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
                for (_, r) in rs.ends_of.iter().filter(|(_, r)| live(r.node, r.serial)) {
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
        self.shards[range_shard(f)]
            .0
            .read()
            .get(&f)
            .map(|m| m.finals.iter().filter(|r| live(r)).cloned().collect())
            .unwrap_or_default()
    }

    /// Files an `At::End(u)` reader of `f`, the dead dropped when the
    /// list is about to grow.
    pub(crate) fn add_end(&self, f: Field, u: UnitId, r: Reader, live: impl Fn(&Reader) -> bool) {
        let mut w = self.shards[range_shard(f)].0.write();
        push_pruned(&mut w.entry(f).or_default().ends_of, (u, r), |(_, r)| live(r));
    }

    /// The `At::End(u)` readers of `f` that still stand.
    pub(crate) fn ends(&self, f: Field, u: UnitId, live: impl Fn(&Reader) -> bool) -> Vec<Reader> {
        self.shards[range_shard(f)]
            .0
            .read()
            .get(&f)
            .map(|m| {
                m.ends_of
                    .iter()
                    .filter(|(v, r)| *v == u && live(r))
                    .map(|(_, r)| r.clone())
                    .collect()
            })
            .unwrap_or_default()
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
