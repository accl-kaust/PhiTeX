//! Continuous versioned merges (docs/round-core.md). A run reads the store
//! without locks and writes nothing to it: when it ends, its worker lands
//! the result (a commit, a park, a check that held, a run again) on the
//! landing list of its node. Workers merge the lists, any number at once,
//! each list in landing order (so one node's items never merge at once),
//! and every merged item publishes the next version. Every write to the
//! store happens in a merge (or in an edit, between evaluations).
//!
//! A merge records the keys it changes (a field of a unit, a unit's
//! structure, a node's output, an environment field) under its version.
//! A landed run carries the version it started at and the keys its reads
//! consulted: it is verified at the merge it lands in by intersecting
//! those keys with the changes since its version, and only a read whose
//! key moved is asked again.

use std::cell::RefCell;
use std::sync::atomic::Ordering;

use super::{DAns, DMode, Pred, Shared, Slot, lock};
use crate::values::{Field, hash_words};

/// A landed item: applied by the merger with the store to itself.
pub(crate) type Land = Box<dyn FnOnce(&Shared) + Send>;

/// One worker's landing list, on cache lines of its own.
#[repr(align(64))]
#[derive(Default)]
pub(crate) struct LandSlot(pub std::sync::Mutex<Vec<Land>>, pub std::sync::Mutex<()>);

/// Landing lists (a node's items all land on one, in order).
pub(crate) const LANDS: usize = 64;

/// The changes the merges made, for verification: each key's last
/// version.
#[derive(Default)]
pub(crate) struct Delta {
    /// Each key's last version and how many merges are writing it now (a
    /// key being written conflicts with every run: its new version is
    /// given when the write is done).
    pub last: crate::FxHashMap<u64, (u64, u32)>,
    /// Changes before this version are no longer known: a run older than
    /// it is verified in full.
    pub base: u64,
}

/// Keys kept for verification, per shard (past this the shard starts over:
/// a run older than that is verified in full).
const DELTA_CAP: usize = 1 << 16;

/// Shards of the delta (by key).
pub(crate) const DELTA_SHARDS: usize = 16;

fn shard_of(k: u64) -> usize {
    (k >> 58) as usize % DELTA_SHARDS
}

thread_local! {
    /// The keys the merge on this thread changed since its last flush
    /// (`None`: not merging, nothing is recorded).
    static CHANGED: RefCell<Option<Vec<u64>>> = const { RefCell::new(None) };
    /// `CHANGED`'s buffer between merges.
    static SPARE: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

// The kinds of keys.
const K_FIELD: u64 = 0x6b66;
const K_UNIT: u64 = 0x6b75;
const K_LITE: u64 = 0x6b6c;
const K_NODE: u64 = 0x6b6e;
const K_ENV: u64 = 0x6b65;

/// A field of a unit: its definitions, contributions and pending sets.
pub(crate) fn kf(u: u32, f: Field) -> u64 {
    hash_words(K_FIELD, &[u64::from(u), f.0.raw()])
}

/// A unit's structure as existing entries see it: labels, frames, kinds,
/// claims, scope effects, entries removed.
pub(crate) fn ku(u: u32) -> u64 {
    hash_words(K_UNIT, &[u64::from(u)])
}

/// Entries added to a unit (sibling views and ranges see them).
pub(crate) fn kl(u: u32) -> u64 {
    hash_words(K_LITE, &[u64::from(u)])
}

/// A node's output, presence and text.
pub(crate) fn kn(s: Slot) -> u64 {
    hash_words(K_NODE, &[u64::from(s)])
}

/// An environment field (the global environment or a unit's).
pub(crate) fn ke(f: Field) -> u64 {
    hash_words(K_ENV, &[f.0.raw()])
}

/// Records that the merge on this thread changed `k`.
impl Shared {
    /// Records that the merge on this thread is changing `k` (before the
    /// change: until this item is done, a run that read `k` is checked).
    pub(crate) fn note(&self, k: u64) {
        let fresh = CHANGED.with(|c| {
            c.borrow_mut().as_mut().is_some_and(|v| {
                if v.contains(&k) {
                    false
                } else {
                    v.push(k);
                    true
                }
            })
        });
        if fresh {
            lock(&self.delta[shard_of(k)]).last.entry(k).or_insert((0, 0)).1 += 1;
        }
    }

    /// The keys the reads of `log` consulted, sorted and deduplicated.
    pub(crate) fn keys_of(&self, log: &[Pred]) -> Vec<u64> {
        let mut ks = Vec::with_capacity(log.len() + 4);
        for p in log {
            match p {
                Pred::Def {
                    f, at, mode, ans, sens, ..
                } => {
                    let u = self.unit_of(*at);
                    ks.push(kf(u, *f));
                    if *sens || *mode != DMode::Use {
                        ks.push(ku(u));
                    }
                    if matches!(ans, DAns::Env(_)) {
                        ks.push(ke(*f));
                    }
                }
                Pred::At { node, .. } => {
                    ks.push(kn(*node));
                    ks.push(ku(self.unit_of(*node)));
                }
                Pred::Out { node, .. } | Pred::App { node, .. } => ks.push(kn(*node)),
                Pred::Range { f, at, .. } => {
                    let u = self.unit_of(*at);
                    ks.push(kf(u, *f));
                    ks.push(ku(u));
                    ks.push(kl(u));
                }
                Pred::Env { f, .. } => ks.push(ke(*f)),
                Pred::IsLoop { .. } => {}
                Pred::Kind { at, opener, .. } => {
                    ks.push(ku(self.unit_of(*at)));
                    if *opener != super::NIL {
                        ks.push(kn(*opener));
                    }
                }
                Pred::Sib { ans, .. } => {
                    if let Some((x, _, _)) = ans {
                        let u = self.unit_of(*x);
                        ks.push(ku(u));
                        ks.push(kl(u));
                        ks.push(kn(*x));
                    }
                }
                Pred::PrevSib { seen, ans } => {
                    for &(x, _, _) in seen {
                        ks.push(kn(x));
                    }
                    if let Some(x) = ans {
                        ks.push(kn(*x));
                    }
                }
                Pred::Claimed { list } => {
                    for &(x, _, _) in list {
                        ks.push(kn(x));
                    }
                }
                Pred::Stream { seen, reads, .. } => {
                    for &(x, _, _) in seen {
                        ks.push(kn(x));
                    }
                    for &(x, f) in reads {
                        ks.push(kf(self.unit_of(x), f));
                        ks.push(ke(f));
                    }
                }
            }
        }
        ks.sort_unstable();
        ks.dedup();
        ks
    }

    /// Whether a change since version `ver` touched one of `keys`.
    pub(crate) fn moved_since(&self, ver: u64, keys: &[u64]) -> bool {
        for k in keys {
            let d = lock(&self.delta[shard_of(*k)]);
            if ver < d.base || d.last.get(k).is_some_and(|&(v, w)| v > ver || w > 0) {
                return true;
            }
        }
        // This merge's own changes so far.
        CHANGED.with(|c| {
            c.borrow()
                .as_ref()
                .is_some_and(|v| v.iter().any(|k| keys.binary_search(k).is_ok()))
        })
    }

    /// The version a run starting now reads at.
    pub(crate) fn version(&self) -> u64 {
        self.ver.load(Ordering::Acquire)
    }

    /// Lands `f` for the merger: merged when the job that landed it ends,
    /// by its worker or by whoever is merging then.
    pub(crate) fn land(&self, s: Slot, f: impl FnOnce(&Shared) + Send + 'static) {
        // The item counts as work until merged: the evaluation is not
        // quiet while it waits.
        self.work.fetch_add(1, Ordering::SeqCst);
        self.landed.fetch_add(1, Ordering::SeqCst);
        // By node: a node's items stay in landing order and are never
        // merged at once (two lists merge concurrently).
        let i = (s as usize).wrapping_mul(0x9E37_79B9) >> 7 & (LANDS - 1);
        lock(&self.lands[i].0).push(Box::new(f));
        self.land_mask.fetch_or(1 << i, Ordering::SeqCst);
        // (Merged by the worker once its job is done: see `Shared::work`.)
    }

    /// Merges what has landed: any number of workers merge at once, each
    /// taking a landing list nobody else is merging (a node's items are
    /// all on one list, so one node's items never merge at once); a list
    /// being merged is left to its merger, never waited for.
    pub(crate) fn merge(&self) {
        while self.landed.load(Ordering::SeqCst) != 0 {
            let mask = self.land_mask.load(Ordering::SeqCst);
            if mask == 0 {
                return;
            }
            let mut took = false;
            for i in 0..LANDS {
                if mask & (1 << i) == 0 {
                    continue;
                }
                let Ok(_g) = self.lands[i].1.try_lock() else { continue };
                self.merge_list(i);
                took = true;
            }
            if !took {
                return;
            }
        }
    }

    /// Merges everything landed (nobody else runs: the step's end).
    pub(crate) fn merge_held(&self) {
        while self.landed.load(Ordering::SeqCst) != 0 {
            for i in 0..LANDS {
                let _g = lock(&self.lands[i].1);
                self.merge_list(i);
            }
        }
    }

    /// Merges landing list `i` (its merge lock held).
    fn merge_list(&self, i: usize) {
        loop {
            self.land_mask.fetch_and(!(1 << i), Ordering::SeqCst);
            let items = std::mem::take(&mut *lock(&self.lands[i].0));
            if items.is_empty() {
                // Landed after the mask was cleared: its lander set it again.
                return;
            }
            let n = items.len();
            // (This thread's buffer, kept from list to list.)
            CHANGED.with(|c| *c.borrow_mut() = Some(SPARE.with(|sp| std::mem::take(&mut *sp.borrow_mut()))));
            let t0 = std::time::Instant::now();
            for it in items {
                it(self);
                self.flush_changes();
            }
            CHANGED.with(|c| {
                if let Some(mut v) = c.borrow_mut().take() {
                    v.clear();
                    SPARE.with(|sp| *sp.borrow_mut() = v);
                }
            });
            let dt = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(0);
            self.c.commit_ns.fetch_add(dt, Ordering::Relaxed);
            self.c.merge_ns.fetch_add(dt, Ordering::Relaxed);
            self.c.merges.fetch_add(1, Ordering::Relaxed);
            self.c.merged.fetch_add(n as u64, Ordering::Relaxed);
            self.landed.fetch_sub(n, Ordering::SeqCst);
            let w = self.work.fetch_sub(n, Ordering::SeqCst);
            if w == n && self.qlen.load(Ordering::SeqCst) == 0 {
                // Perhaps quiet: the sleepers look.
                self.wake_idle(true);
            }
        }
    }

    /// Publishes this item's changes under a new version (after its
    /// writes: a run that reads the new version sees them).
    fn flush_changes(&self) {
        let ks = CHANGED.with(|c| c.borrow_mut().as_mut().map(std::mem::take).unwrap_or_default());
        if ks.is_empty() {
            return;
        }
        let ver = self.ver.fetch_add(1, Ordering::AcqRel) + 1;
        for &k in &ks {
            let mut d = lock(&self.delta[shard_of(k)]);
            let e = d.last.entry(k).or_insert((0, 1));
            e.0 = ver;
            e.1 = e.1.saturating_sub(1);
            if d.last.len() > DELTA_CAP {
                // (Only keys nobody is writing go.)
                d.last.retain(|_, e| e.1 > 0);
                d.base = ver;
            }
        }
        // (Its buffer goes back for the next item: no regrowth per item.)
        let mut ks = ks;
        ks.clear();
        CHANGED.with(|c| {
            if let Some(v) = c.borrow_mut().as_mut()
                && v.is_empty()
            {
                *v = ks;
            }
        });
    }

    /// Forgets the delta (between evaluations no run is in flight).
    pub(crate) fn reset_delta(&self) {
        let base = self.ver.load(Ordering::Acquire);
        for d in &self.delta {
            let mut d = lock(d);
            d.last.clear();
            d.base = base;
        }
    }
}
