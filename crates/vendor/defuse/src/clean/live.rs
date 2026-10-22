//! The reader indexes, live: shared by every handle of a world and written
//! by the workers that ran the readers (each files its own reads after its
//! run) as well as by the merge, each entry under its shard's lock. Runs
//! never read them: only the merge does, to find the readers a change
//! reaches, and the filing itself. A world kept for later (a round's) takes
//! a snapshot ([`Readers::snapshot`]).

use std::sync::{Arc, Mutex, RwLock};

use super::claims::Windows;
use super::core::lock;
use super::ptree::{Measure, NoMeasure, PMap};
use super::world::{Id, Pos, ReadKey};
use crate::values::{Field, FxHashMap};

const SHARDS: usize = 64;

/// A renumbering epoch never used before (by any world: worlds kept apart
/// never share one).
pub(crate) fn fresh_epoch() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A map sharded by key hash, each shard behind its own lock.
pub(crate) struct Sharded<K, V> {
    shards: Box<[Mutex<FxHashMap<K, V>>]>,
}

impl<K: std::hash::Hash + Eq + Clone, V: Clone + Default> Sharded<K, V> {
    fn new() -> Self {
        Sharded {
            shards: (0..SHARDS).map(|_| Mutex::new(FxHashMap::default())).collect(),
        }
    }

    fn shard(&self, k: &K) -> &Mutex<FxHashMap<K, V>> {
        use std::hash::{BuildHasher, BuildHasherDefault};
        let h = BuildHasherDefault::<crate::values::FxHasher>::default().hash_one(k);
        &self.shards[(h as usize) % SHARDS]
    }

    /// `k`'s value as it is now (a cheap copy: persistent contents).
    pub(crate) fn get(&self, k: &K) -> Option<V> {
        lock(self.shard(k)).get(k).cloned()
    }

    /// Changes `k`'s value (made if new).
    pub(crate) fn with<R>(&self, k: K, f: impl FnOnce(&mut V) -> R) -> R {
        let mut g = lock(self.shard(&k));
        f(g.entry(k).or_default())
    }

    /// Changes `k`'s value if there is one.
    pub(crate) fn with_if<R>(&self, k: &K, f: impl FnOnce(&mut V) -> R) -> Option<R> {
        let mut g = lock(self.shard(k));
        g.get_mut(k).map(f)
    }

    pub(crate) fn insert(&self, k: K, v: V) {
        lock(self.shard(&k)).insert(k, v);
    }

    /// Every key (any order).
    pub(crate) fn keys(&self) -> Vec<K> {
        self.shards.iter().flat_map(|s| lock(s).keys().cloned().collect::<Vec<_>>()).collect()
    }

    fn snapshot(&self) -> Self {
        Sharded {
            shards: self.shards.iter().map(|s| Mutex::new(lock(s).clone())).collect(),
        }
    }
}

/// A field's readers in a unit: by the position they read at, and its
/// range readers by the end of their window and the reader (the window's
/// start).
#[derive(Clone, Default)]
pub(crate) struct FieldReaders {
    pub readers: PMap<(Pos, Id), ()>,
    pub range_readers: Ranges,
}

/// Range readers by the end of their window, the reader and the window's
/// start (a reader's windows differing in start only are entries apart),
/// each with the window's start; summed by the least start (what starts
/// at or past a point is passed whole).
pub(crate) type Ranges = PMap<(Pos, Id, Pos), Pos, Pos>;

/// [`Ranges`]'s summary: the least window start.
pub(crate) struct LeastStart;

impl Measure<(Pos, Id, Pos), Pos> for LeastStart {
    type S = Pos;
    fn zero(&self) -> Pos {
        Pos::MAX
    }
    fn of(&self, _: &(Pos, Id, Pos), lo: &Pos) -> Pos {
        *lo
    }
    fn add(&self, a: &Pos, b: &Pos) -> Pos {
        (*a).min(*b)
    }
}

impl FieldReaders {
    /// The range readers whose window `(lo, hi)` holds position `q`
    /// (`lo < q < hi`), in window-end order: the windows after `q` walked
    /// but for runs of them that all start at or past it.
    pub(crate) fn over(&self, q: Pos) -> Vec<Id> {
        let mut out = Vec::new();
        self.range_readers.visit_where(std::ops::Bound::Excluded(&(q, Id::MAX, Pos::MAX)), &LeastStart, &|&lo| lo >= q, &mut |k, &lo| {
            if lo < q {
                out.push(k.1);
            }
        });
        out
    }
}

/// A unit's structure readers: of the scope structure at their position,
/// and of following and earlier siblings with their windows' reach.
#[derive(Clone, Default)]
pub(crate) struct UnitReaders {
    pub kind_readers: PMap<(Pos, Id), ()>,
    pub fwd_readers: Windows,
    pub back_readers: Windows,
}

/// The entries a node's reads are filed as: every entry the index holds
/// for it (sorted, distinct). A worker adds what its run read before the
/// merge commits it, so they may be more than its committed reads need
/// (a run not committed); never fewer.
#[derive(Clone, Default)]
pub(crate) enum Slot {
    #[default]
    None,
    Keys(Arc<[ReadKey]>),
    /// The node is gone: a late worker files nothing.
    Gone,
}

impl Slot {
    pub(crate) fn keys(&self) -> &[ReadKey] {
        match self {
            Slot::Keys(k) => k,
            _ => &[],
        }
    }
}

struct Inner {
    fields: Sharded<(u32, Field), FieldReaders>,
    units: Sharded<u32, UnitReaders>,
    /// Readers of a node's output (or of its being there).
    outs: Sharded<Id, PMap<Id, ()>>,
    /// The entries each node's reads are filed as.
    filed: Sharded<Id, Slot>,
    /// The last renumbering's epoch (`World::relabels`): a worker's keys
    /// made at another are stale. A renumbering holds it for writing while
    /// it moves the entries; a worker filing holds it for reading.
    relabels: RwLock<u64>,
}

/// The reader indexes of a world (see the module doc). Made on first use
/// (a world taken out for a moment is a default one).
#[derive(Clone, Default)]
pub(crate) struct Readers(Arc<std::sync::OnceLock<Inner>>);

impl Inner {
    fn new() -> Self {
        Inner {
            fields: Sharded::new(),
            units: Sharded::new(),
            outs: Sharded::new(),
            filed: Sharded::new(),
            relabels: RwLock::new(0),
        }
    }
}

impl Readers {
    #[inline]
    fn i(&self) -> &Inner {
        self.0.get_or_init(Inner::new)
    }

    /// A copy that goes its own way from here (a kept round's world).
    pub(crate) fn snapshot(&self) -> Readers {
        let i = self.i();
        let r = *i.relabels.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        let n = Inner {
            fields: i.fields.snapshot(),
            units: i.units.snapshot(),
            outs: i.outs.snapshot(),
            filed: i.filed.snapshot(),
            relabels: RwLock::new(r),
        };
        let c = std::sync::OnceLock::new();
        let _ = c.set(n);
        Readers(Arc::new(c))
    }

    pub(crate) fn field(&self, u: u32, f: Field) -> Option<FieldReaders> {
        self.i().fields.get(&(u, f))
    }

    pub(crate) fn with_field<R>(&self, u: u32, f: Field, g: impl FnOnce(&mut FieldReaders) -> R) -> R {
        self.i().fields.with((u, f), g)
    }

    /// The fields of unit `u` with readers, in field order.
    pub(crate) fn fields_of(&self, u: u32) -> Vec<Field> {
        let mut v: Vec<Field> = self.i().fields.keys().into_iter().filter(|k| k.0 == u).map(|k| k.1).collect();
        v.sort_unstable();
        v
    }

    pub(crate) fn unit(&self, u: u32) -> Option<UnitReaders> {
        self.i().units.get(&u)
    }

    pub(crate) fn with_unit<R>(&self, u: u32, g: impl FnOnce(&mut UnitReaders) -> R) -> R {
        self.i().units.with(u, g)
    }

    pub(crate) fn outs(&self, of: Id) -> Option<PMap<Id, ()>> {
        self.i().outs.get(&of)
    }

    pub(crate) fn add_out(&self, of: Id, id: Id) {
        self.i().outs.with(of, |m| {
            m.insert(id, (), &NoMeasure);
        });
    }

    /// Node `of`'s output readers, taken out (it is gone).
    pub(crate) fn take_outs(&self, of: Id) -> Option<PMap<Id, ()>> {
        lock(self.i().outs.shard(&of)).remove(&of)
    }

    pub(crate) fn drop_out(&self, of: Id, id: Id) {
        self.i().outs.with_if(&of, |m| {
            m.remove(&id, &NoMeasure);
        });
    }

    /// Node `id`'s filed entries, held while `f` changes them (and the
    /// index entries with them).
    pub(crate) fn with_slot<R>(&self, id: Id, f: impl FnOnce(&mut Slot) -> R) -> R {
        self.i().filed.with(id, f)
    }

    /// Forgets the slots of nodes gone (no worker runs: none files one
    /// again).
    pub(crate) fn purge_gone(&self) {
        for s in self.i().filed.shards.iter() {
            lock(s).retain(|_, v| !matches!(v, Slot::Gone));
        }
    }

    /// Every slot's entries mapped by `f` (a renumbering).
    pub(crate) fn remap_slots(&self, f: impl Fn(ReadKey) -> ReadKey) {
        for s in self.i().filed.shards.iter() {
            for v in lock(s).values_mut() {
                if let Slot::Keys(ks) = v {
                    let mut n: Vec<ReadKey> = ks.iter().map(|&k| f(k)).collect();
                    n.sort_unstable();
                    n.dedup();
                    *ks = n.into();
                }
            }
        }
    }

    /// The renumbering epoch, held for reading (a filing at this epoch).
    pub(crate) fn relabels(&self) -> std::sync::RwLockReadGuard<'_, u64> {
        self.i().relabels.read().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The renumbering epoch, held for writing (a renumbering).
    pub(crate) fn relabels_mut(&self) -> std::sync::RwLockWriteGuard<'_, u64> {
        self.i().relabels.write().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Every unit's and field's keys (for checks).
    #[allow(dead_code, reason = "checks")]
    pub(crate) fn all_fields(&self) -> Vec<(u32, Field)> {
        self.i().fields.keys()
    }

    pub(crate) fn set_unit(&self, u: u32, r: UnitReaders) {
        self.i().units.insert(u, r);
    }

    pub(crate) fn set_field(&self, u: u32, f: Field, r: FieldReaders) {
        self.i().fields.insert((u, f), r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `FieldReaders::over` finds what a walk of every window after the
    /// point finds.
    #[test]
    fn over_is_the_walk() {
        let mut r = FieldReaders::default();
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        for id in 1..400u64 {
            let lo = next(1000);
            let hi = lo + 1 + next(300);
            r.range_readers.insert((hi, id, lo), lo, &LeastStart);
            if next(5) == 0 {
                let k = (next(1300), next(400), next(1000));
                r.range_readers.remove(&k, &LeastStart);
            }
        }
        for q in 0..1400 {
            let walk: Vec<Id> = r
                .range_readers
                .range(std::ops::Bound::Excluded(&(q, Id::MAX, Pos::MAX)), std::ops::Bound::Unbounded)
                .filter(|(_, lo)| **lo < q)
                .map(|(k, _)| k.1)
                .collect();
            assert_eq!(r.over(q), walk, "at {q}");
        }
    }
}
