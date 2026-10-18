//! The per-round memo (`LoopStart::PerRound`): what each node committed in
//! each round of the last evaluation, offered to the same round of the
//! next one in place of a run.
//!
//! A node is offered its entry only when it would run: its text unchanged
//! and the entry the last it committed at or before the round (a node that
//! committed once serves every later round from one entry). The entry is
//! committed as a run's result would be, with every read checked again
//! against the store (version 0): reads that hold make it the round's
//! result (a hit); one that broke sends the node to run (marked tried for
//! the round), one that waits parks it as a run's would. The memo changes
//! how much runs, never what is committed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::ctx::RunOut;
use super::{Pred, Shared, Slot, lock};
use crate::values::{FxHashMap, FxHashSet, ValueId};

/// The final phase's round number in the memo.
pub(crate) const FINAL: u32 = u32::MAX;

/// A committed result: the run's output and what it made, its anchors
/// as node ids (a slot may be reused).
pub(crate) struct Entry {
    ro: RunOut,
    anchors: Vec<Option<u64>>,
    out: ValueId,
    /// A final node's (its loop-back reads make no round).
    fin: bool,
}

/// Per node id, its committed results by round (ascending).
type Rounds = FxHashMap<u64, Vec<(u32, Arc<Entry>)>>;

#[derive(Default)]
pub(crate) struct Memo {
    /// Recording and offering (`LoopStart::PerRound`).
    pub on: AtomicBool,
    /// The round being run (1-based; `FINAL` in the final phase).
    pub round: AtomicU32,
    /// The last evaluation's results.
    prev: std::sync::RwLock<Rounds>,
    /// This evaluation's, so far.
    cur: std::sync::Mutex<Rounds>,
    /// Nodes whose entry broke this round (they run).
    tried: std::sync::Mutex<FxHashSet<Slot>>,
    /// Hits (entries committed) so far.
    pub hits: AtomicU64,
    /// Edited since the last evaluation.
    pub edited: AtomicBool,
    /// An edit since the last evaluation was structural (an insert, a
    /// removal, a text whose declarations changed): no quick path.
    pub structural: AtomicBool,
    /// The evaluation runs the quick path (from the last fixed point; see
    /// `quick_holds`).
    pub quick: AtomicBool,
    /// The last evaluation's rounds and capped fields.
    pub prev_rounds: AtomicU64,
    pub prev_capped: std::sync::Mutex<Option<Vec<crate::values::Field>>>,
    /// Per loop-back change of this evaluation, in order: the round it
    /// ended, the fields changed, and whether a round was counted for it
    /// (one is only when a reader runs); and the last evaluation's.
    changes: std::sync::Mutex<Vec<Change>>,
    prev_changes: std::sync::Mutex<Vec<Change>>,
    /// Who reads what in the last evaluation (for `quick_holds`).
    ix: std::sync::Mutex<Index>,
    /// The bytes held (computed when the memo is replaced).
    bytes: AtomicU64,
}

/// A loop-back change at the end of a round: the fields, whether a round
/// was counted for it, and the live readers that made it count.
pub(crate) struct Change {
    counted: bool,
    readers: Vec<u64>,
}

/// The last evaluation's reads, inverted: built when its memo is kept,
/// added to (never pruned) by quick paths.
#[derive(Default)]
struct Index {
    /// Entries (any round) reading a field.
    by_field: FxHashMap<crate::values::Field, Vec<u64>>,
    /// Entries naming a node in a read.
    by_slot: FxHashMap<Slot, Vec<u64>>,
    /// Nodes that published in some round.
    publishers: FxHashSet<u64>,
    /// The fields a node contributed to in some round, and those it
    /// defined.
    contribs_of: FxHashMap<u64, Vec<crate::values::Field>>,
    defs_of: FxHashMap<u64, Vec<crate::values::Field>>,
    /// Nodes that read a loop-back value in some round.
    loop_readers: FxHashSet<u64>,
    /// Nodes known at their last round only (a quick path ran them): not
    /// counted as a change's reader.
    uncertain: FxHashSet<u64>,
}

impl Index {
    fn add(&mut self, id: u64, e: &Entry) {
        self.contribs_of
            .entry(id)
            .or_default()
            .extend(e.ro.contribs.iter().map(|c| c.0));
        self.defs_of
            .entry(id)
            .or_default()
            .extend(e.ro.defs.iter().map(|d| d.f));
        if !e.ro.pubs.is_empty() {
            self.publishers.insert(id);
        }
        for p in &e.ro.log {
            read_keys(p, &mut |k| match k {
                Key::F(f) => self.by_field.entry(f).or_default().push(id),
                Key::N(x) => self.by_slot.entry(x).or_default().push(id),
            });
            if matches!(
                p,
                Pred::Def {
                    ans: super::DAns::Loop(_),
                    ..
                }
            ) && !e.fin
            {
                self.loop_readers.insert(id);
            }
        }
    }
}

impl Memo {
    /// A loop-back change at the end of the current round.
    pub(crate) fn note_change(&self, _ch: Vec<crate::values::Field>, counted: bool, readers: Vec<u64>) {
        if self.on.load(Ordering::Relaxed) {
            lock(&self.changes).push(Change { counted, readers });
        }
    }

    /// A new round: entries may be offered again.
    pub(crate) fn next_round(&self, k: u32) {
        self.round.store(k, Ordering::Release);
        lock(&self.tried).clear();
    }

    /// A new evaluation: no changes noted yet.
    pub(crate) fn begin(&self) {
        lock(&self.changes).clear();
    }

    /// The evaluation ended well: its results are the next one's memo.
    pub(crate) fn finish(&self, rounds: u64, capped: Option<Vec<crate::values::Field>>) {
        let cur = std::mem::take(&mut *lock(&self.cur));
        *self.prev.write().unwrap_or_else(std::sync::PoisonError::into_inner) = cur;
        self.prev_rounds.store(rounds, Ordering::Relaxed);
        *lock(&self.prev_capped) = capped;
        let changes = std::mem::take(&mut *lock(&self.changes));
        // The index of the kept memo.
        let prev = self.prev.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut ix = Index::default();
        for (&id, v) in prev.iter() {
            for (_, e) in v {
                ix.add(id, e);
            }
        }
        drop(prev);
        *lock(&self.ix) = ix;
        *lock(&self.prev_changes) = changes;
        self.bytes.store(self.measure(), Ordering::Relaxed);
    }

    /// Whether the quick path may be tried: only value edits (same
    /// declarations) since a memoised evaluation.
    pub(crate) fn quick_eligible(&self) -> bool {
        self.edited.load(Ordering::Acquire)
            && !self.structural.load(Ordering::Acquire)
            && self.prev_rounds.load(Ordering::Relaxed) > 0
            && !self
                .prev
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
    }

    /// The quick path held: the rounds are the last evaluation's, the nodes
    /// it committed replace their entries for the last round.
    pub(crate) fn finish_quick(&self) {
        let cur = std::mem::take(&mut *lock(&self.cur));
        let mut prev = self.prev.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut ix = lock(&self.ix);
        for (id, v) in cur {
            let Some((k, e)) = v.into_iter().last() else { continue };
            ix.add(id, &e);
            ix.uncertain.insert(id);
            let p = prev.entry(id).or_default();
            match p.last_mut() {
                Some(last) if last.0 == k => last.1 = e,
                _ => p.push((k, e)),
            }
        }
    }

    /// The evaluation failed: no memo (the next one runs what it must).
    pub(crate) fn drop_all(&self) {
        lock(&self.cur).clear();
        lock(&self.changes).clear();
        self.prev_rounds.store(0, Ordering::Relaxed);
        self.prev
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        *lock(&self.ix) = Index::default();
        self.bytes.store(0, Ordering::Relaxed);
    }

    /// This evaluation's results so far.
    pub(crate) fn cur_ref(&self) -> &std::sync::Mutex<Rounds> {
        &self.cur
    }

    /// An estimate of the bytes held.
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    fn measure(&self) -> u64 {
        let prev = self.prev.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut b = 0usize;
        for v in prev.values() {
            b += 32 + v.len() * 16;
            for (_, e) in v {
                let r = &e.ro;
                b += std::mem::size_of::<Entry>()
                    + r.log.len() * std::mem::size_of::<Pred>()
                    + r.defs.len() * std::mem::size_of::<super::Def>()
                    + (r.contribs.len() + r.pubs.len() + r.env.len()) * 16
                    + (r.kids.len() + r.conts.len()) * std::mem::size_of::<super::order::Spec>()
                    + r.args.len() * 8;
            }
        }
        b as u64
    }
}

impl Shared {
    /// Records a commit of `s` in the current round (the last of a round
    /// wins).
    pub(crate) fn memo_record(&self, s: Slot, ro: &RunOut, out: ValueId) {
        let k = self.memo.round.load(Ordering::Acquire);
        let mut ro = ro.clone();
        let anchors = ro.anchors.iter().map(|a| a.map(|x| self.node_id(x).0)).collect();
        ro.run_ns = 0;
        ro.wait = None;
        ro.keys = Vec::new();
        ro.pread = false;
        ro.narrowed = None;
        let fin = self.rec(s).data().stat.fin();
        let e = Arc::new(Entry { ro, anchors, out, fin });
        let id = self.node_id(s).0;
        let mut cur = lock(&self.memo.cur);
        let v = cur.entry(id).or_default();
        match v.last_mut() {
            Some(last) if last.0 == k => last.1 = e,
            _ => v.push((k, e)),
        }
    }

    /// Offers `s` its entry for the round in place of a run: `true` when
    /// one was landed (committed if its reads hold, else `s` runs).
    pub(crate) fn memo_offer(&self, s: Slot) -> bool {
        let k = self.memo.round.load(Ordering::Acquire);
        if lock(&self.memo.tried).contains(&s) {
            return false;
        }
        let id = self.node_id(s).0;
        let e = {
            let prev = self.memo.prev.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(v) = prev.get(&id) else { return false };
            // The last entry at or before the round (the final phase's only
            // in the final phase).
            let e = if k == FINAL {
                v.last().map(|x| &x.1)
            } else {
                v.iter().rev().find(|x| x.0 <= k).map(|x| &x.1)
            };
            let Some(e) = e else { return false };
            e.clone()
        };
        let r = self.rec(s);
        {
            let d = r.data();
            if d.op != e.ro.op || d.args != e.ro.args {
                return false;
            }
        }
        // Anchors back to slots (a node gone or its slot reused: no offer).
        let mut anchors = Vec::with_capacity(e.anchors.len());
        for a in &e.anchors {
            match a {
                None => anchors.push(None),
                Some(id) => match self.slot_of(*id) {
                    Some(x) if self.rec(x).st() != super::S_FREE => anchors.push(Some(x)),
                    _ => return false,
                },
            }
        }
        let mut ro = e.ro.clone();
        ro.me = s;
        ro.anchors = anchors;
        // Its reads as committed reads, all of them checked at the commit.
        for p in &mut ro.log {
            if let Pred::Def { spec, .. } = p {
                *spec = None;
            }
        }
        ro.ver = 0;
        ro.keys = self.keys_of(&ro.log);
        let out = e.out;
        let u = self.unit_of(s);
        let lgen = r.lgen.load(Ordering::Acquire).wrapping_add(1) & 0x7fff_ffff;
        r.lgen.store(lgen, Ordering::Release);
        self.subscribe(s, u, lgen, &ro.log);
        self.land(s, move |sh| sh.memo_commit(ro, out));
        true
    }

    /// After the quick path (the edits evaluated from the last fixed point):
    /// whether no committed change can reach a loop-back publication in
    /// any round of the last evaluation, so its round sequence stands and
    /// the state reached is the cold one. Conservative: what the nodes
    /// committed now and in any round before (fields defined, contributed,
    /// published), read by any entry's log in any round (by field, or by a
    /// node it names), closed transitively; a publisher reached, a changed
    /// unfolding, effect or claim, or a node without an entry, fails it.
    pub(crate) fn quick_holds(&self) -> bool {
        let cur = lock(&self.memo.cur);
        let prev = self.memo.prev.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        let ix = lock(&self.memo.ix);
        let mut seen: FxHashSet<u64> = FxHashSet::default();
        let mut work: Vec<Key> = Vec::new();
        let mut new_loop_read = false;
        for (id, v) in cur.iter() {
            let Some((_, new)) = v.last() else { continue };
            let Some((_, last)) = prev.get(id).and_then(|o| o.last()) else {
                return false;
            };
            if !new.ro.pubs.is_empty() || ix.publishers.contains(id) {
                return false;
            }
            if new.ro.effect != last.ro.effect
                || new.ro.claim != last.ro.claim
                || new.ro.kids.len() != last.ro.kids.len()
                || new.ro.conts.len() != last.ro.conts.len()
                || new.ro.kids.iter().zip(&last.ro.kids).any(|(a, b)| a.id() != b.id())
                || new.ro.conts.iter().zip(&last.ro.conts).any(|(a, b)| a.id() != b.id())
            {
                return false;
            }
            new_loop_read |= !new.fin && reads_loop(&new.ro.log, None);
            // Its contributions reach range readers by window (by field
            // here); its definitions reach readers by the node that
            // answered them, and by field where its set of fields moved.
            work.extend(new.ro.contribs.iter().map(|c| Key::F(c.0)));
            let old_defs: FxHashSet<crate::values::Field> = ix
                .defs_of
                .get(id)
                .map(|v| v.iter().copied().collect())
                .unwrap_or_default();
            let new_defs: FxHashSet<crate::values::Field> = new.ro.defs.iter().map(|d| d.f).collect();
            work.extend(old_defs.symmetric_difference(&new_defs).map(|&f| Key::F(f)));
            seen.insert(*id);
        }
        // Readers in any round, to a fixed point.
        for id in seen.clone() {
            self.quick_keys(&ix, id, &mut work);
        }
        let mut done_f: FxHashSet<crate::values::Field> = FxHashSet::default();
        let mut done_n: FxHashSet<Slot> = FxHashSet::default();
        while let Some(k) = work.pop() {
            let readers = match k {
                Key::F(f) => {
                    if !done_f.insert(f) {
                        continue;
                    }
                    ix.by_field.get(&f)
                }
                Key::N(x) => {
                    if !done_n.insert(x) {
                        continue;
                    }
                    ix.by_slot.get(&x)
                }
            };
            for &w in readers.into_iter().flatten() {
                if !seen.insert(w) {
                    continue;
                }
                if ix.publishers.contains(&w) {
                    return false;
                }
                self.quick_keys(&ix, w, &mut work);
            }
        }
        // The round count: a counted round stands while a reader of the
        // change it followed is out of reach (it read the same in every
        // round); an uncounted change stays so while nothing in reach reads
        // a loop-back at all.
        let in_reach_reads_loops = new_loop_read || seen.iter().any(|id| ix.loop_readers.contains(id));
        for c in lock(&self.memo.prev_changes).iter() {
            let (counted, readers) = (c.counted, &c.readers);
            if !counted {
                if in_reach_reads_loops {
                    return false;
                }
                continue;
            }
            if !readers.iter().any(|r| !seen.contains(r) && !ix.uncertain.contains(r)) {
                return false;
            }
        }
        true
    }

    /// What a node in reach can change for others: its contributions (by
    /// field), the reads naming it, and, when its set of definitions is not
    /// static, its defined fields by field.
    fn quick_keys(&self, ix: &Index, id: u64, work: &mut Vec<Key>) {
        if let Some(fs) = ix.contribs_of.get(&id) {
            work.extend(fs.iter().map(|&f| Key::F(f)));
        }
        let Some(x) = self.slot_of(id) else {
            // (Gone: anything it defined.)
            if let Some(fs) = ix.defs_of.get(&id) {
                work.extend(fs.iter().map(|&f| Key::F(f)));
            }
            return;
        };
        work.push(Key::N(x));
        if self.rec(x).data().stat.defs.is_none()
            && let Some(fs) = ix.defs_of.get(&id)
        {
            work.extend(fs.iter().map(|&f| Key::F(f)));
        }
    }

    /// The merger's part of an offer: committed as a run's result, or the
    /// node runs (it waits, or its entry broke and is not offered again
    /// this round).
    fn memo_commit(&self, mut o: RunOut, out: ValueId) {
        let s = o.me;
        let r = self.rec(s);
        if (r.has(super::F_DOOM_FREE | super::F_DOOM_INERT) && self.doomed(s)) || r.st() != super::S_RUN {
            return;
        }
        if self.commit(&mut o, out) {
            self.memo.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            let w = o.wait.take();
            if w.is_none() {
                lock(&self.memo.tried).insert(s);
            }
            self.not_committed(s, w);
        }
    }
}

/// Whether a log reads a loop-back value (of one of `fields`, if given).
fn reads_loop(log: &[Pred], fields: Option<&Vec<crate::values::Field>>) -> bool {
    log.iter().any(|p| match p {
        Pred::Def {
            f,
            ans: super::DAns::Loop(_),
            ..
        } => fields.is_none_or(|fs| fs.contains(f)),
        _ => false,
    })
}

/// What a read depends on: a field, or a node it names.
enum Key {
    F(crate::values::Field),
    N(Slot),
}

/// Calls `k` with every field and node `p` names.
fn read_keys(p: &Pred, k: &mut impl FnMut(Key)) {
    match p {
        Pred::Def { f, ans, sites, .. } => {
            k(Key::F(*f));
            if let super::DAns::Site { node, .. } = ans {
                k(Key::N(*node));
            }
            for (x, _) in sites.iter() {
                k(Key::N(*x));
            }
        }
        Pred::Range { f, last, .. } => {
            k(Key::F(*f));
            k(Key::N(*last));
        }
        Pred::At { node, .. } | Pred::Out { node, .. } | Pred::App { node, .. } => k(Key::N(*node)),
        Pred::Kind { opener, .. } => k(Key::N(*opener)),
        Pred::Sib { ans: Some(a), .. } => k(Key::N(a.0)),
        Pred::PrevSib { seen, ans } => {
            for x in seen {
                k(Key::N(x.0));
            }
            if let Some(a) = ans {
                k(Key::N(*a));
            }
        }
        Pred::Claimed { list } => {
            for x in list {
                k(Key::N(x.0));
            }
        }
        Pred::Stream { seen, reads, .. } => {
            for x in seen {
                k(Key::N(x.0));
            }
            for (x, f) in reads {
                k(Key::N(*x));
                k(Key::F(*f));
            }
        }
        // (Environment and loop-back reads are not a node's to change.)
        _ => {}
    }
}
