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
    /// The node id behind every slot its reads name (in visiting order):
    /// offered, the reads are translated to the nodes' current slots and
    /// generations (a node a run places is made again, same id, in a new
    /// slot, when its placer runs again), their values still checked.
    read_ids: Vec<u64>,
    out: ValueId,
    /// A final node's (its loop-back reads make no round).
    fin: bool,
}

impl Entry {
    /// Committed the same as `o`: the same reads (by node id) with the same
    /// answers, and the same results.
    fn same(&self, o: &Entry) -> bool {
        if self.out != o.out
            || self.read_ids != o.read_ids
            || self.ro.defs != o.ro.defs
            || self.ro.contribs != o.ro.contribs
            || self.ro.pubs != o.ro.pubs
            || self.ro.env != o.ro.env
            || self.ro.kind != o.ro.kind
            || self.ro.kinds != o.ro.kinds
            || self.ro.log.len() != o.ro.log.len()
        {
            return false;
        }
        self.ro.log.iter().zip(&o.ro.log).all(|(a, b)| {
            let (mut a, mut b) = (a.clone(), b.clone());
            for p in [&mut a, &mut b] {
                visit_slots(p, &mut |x, g| {
                    *x = 0;
                    if let Some(g) = g {
                        *g = 0;
                    }
                });
            }
            a == b
        })
    }
}

/// The loop-back fields a log reads.
fn loop_fields(log: &[Pred]) -> Vec<crate::values::Field> {
    log.iter()
        .filter_map(|p| match p {
            Pred::Def {
                f,
                ans: super::DAns::Loop(_),
                ..
            } => Some(*f),
            _ => None,
        })
        .collect()
}

/// The fields whose (field, value) lists differ between `a` and `b`.
fn changed_fields(
    a: &[(crate::values::Field, ValueId)],
    b: &[(crate::values::Field, ValueId)],
) -> Vec<crate::values::Field> {
    if a == b {
        return Vec::new();
    }
    let mut fs: Vec<crate::values::Field> = a.iter().chain(b).map(|x| x.0).collect();
    fs.sort_unstable();
    fs.dedup();
    fs.retain(|f| {
        let pa: Vec<_> = a.iter().filter(|x| x.0 == *f).collect();
        let pb: Vec<_> = b.iter().filter(|x| x.0 == *f).collect();
        pa != pb
    });
    fs
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
    /// Entries not reused this evaluation: (node id, round, why).
    pub misses: std::sync::Mutex<Vec<(u64, u32, String)>>,
    /// Hits (entries committed) so far.
    pub hits: AtomicU64,
    /// Edited since the last evaluation.
    pub edited: AtomicBool,
    /// An edit since the last evaluation the quick path cannot judge (a
    /// claimed node's text, a text whose scope effect, claim or finality
    /// changed): the rounds run.
    pub structural: AtomicBool,
    /// Since the last evaluation: nodes freed (id, slot, parent, and
    /// whether its text had a scope effect or a claim), and the parents
    /// that got a node inserted.
    pub removed: std::sync::Mutex<Vec<(u64, Slot, Slot, bool)>>,
    pub insert_parents: std::sync::Mutex<Vec<Slot>>,
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
    fields: Vec<crate::values::Field>,
    counted: bool,
    readers: Vec<u64>,
}

/// The last evaluation's reads, inverted: built when its memo is kept,
/// added to (never pruned) by quick paths.
#[derive(Default)]
struct Index {
    /// Entries (any round) reading a field: by use (the nearest visible
    /// definition), and otherwise (a sequence, a range, a stream).
    by_field: FxHashMap<crate::values::Field, Vec<u64>>,
    /// (Slots: a reader's position is looked at, a freed slot skipped.)
    by_field_use: FxHashMap<crate::values::Field, Vec<Slot>>,
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
    /// Parents of the siblings a read walked or named (a sibling, a claim,
    /// a stream, a backward scan): an insert or removal under one of them
    /// shifts what it saw.
    sib_parents: FxHashSet<Slot>,
}

impl Index {
    fn add(&mut self, id: u64, e: &Entry, parent: &dyn Fn(Slot) -> Slot) {
        for p in &e.ro.log {
            let mut sib = |x: Slot| {
                self.sib_parents.insert(parent(x));
            };
            match p {
                Pred::Sib { ans: Some(a), .. } => sib(a.0),
                Pred::PrevSib { seen, ans } => {
                    seen.iter().for_each(|x| sib(x.0));
                    ans.iter().for_each(|&x| sib(x));
                }
                Pred::Claimed { list } => list.iter().for_each(|x| sib(x.0)),
                Pred::Stream { seen, .. } => seen.iter().for_each(|x| sib(x.0)),
                _ => {}
            }
        }
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
            if let Pred::Def {
                f,
                mode: super::DMode::Use,
                ..
            } = p
            {
                self.by_field_use.entry(*f).or_default().push(e.ro.me);
            }
            read_keys(p, &mut |k| match k {
                Key::F(f) => self.by_field.entry(f).or_default().push(id),
                Key::N(x) => self.by_slot.entry(x).or_default().push(id),
                Key::F2(_) | Key::R(_) => {}
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
    pub(crate) fn note_change(&self, ch: Vec<crate::values::Field>, counted: bool, readers: Vec<u64>) {
        if self.on.load(Ordering::Relaxed) {
            lock(&self.changes).push(Change {
                fields: ch,
                counted,
                readers,
            });
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
        lock(&self.misses).clear();
    }

    /// The evaluation ended well: its results are the next one's memo.
    pub(crate) fn finish(&self, rounds: u64, capped: Option<Vec<crate::values::Field>>, parent: &dyn Fn(Slot) -> Slot) {
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
                ix.add(id, e, parent);
            }
        }
        lock(&self.removed).clear();
        lock(&self.insert_parents).clear();
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
    pub(crate) fn finish_quick(&self, parent: &dyn Fn(Slot) -> Slot) {
        let cur = std::mem::take(&mut *lock(&self.cur));
        let mut prev = self.prev.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut ix = lock(&self.ix);
        for (id, v) in cur {
            let Some((k, e)) = v.into_iter().last() else { continue };
            ix.add(id, &e, parent);
            ix.uncertain.insert(id);
            // (Round-invariant, or new, when the quick path held: one
            // entry serves every round.)
            let p = prev.entry(id).or_default();
            let k = p.first().map_or(if k == FINAL { k } else { 1 }, |x| x.0);
            *p = vec![(k, e)];
        }
        // (A node removed is no change's reader any more.)
        for &(id, ..) in lock(&self.removed).iter() {
            ix.uncertain.insert(id);
        }
        drop(ix);
        drop(prev);
        lock(&self.removed).clear();
        lock(&self.insert_parents).clear();
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
        lock(&self.removed).clear();
        lock(&self.insert_parents).clear();
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
        let mut read_ids = Vec::new();
        for p in &mut ro.log {
            visit_slots(p, &mut |x, _| {
                read_ids.push(if *x == super::NIL { u64::MAX } else { self.node_id(*x).0 })
            });
        }
        let e = Arc::new(Entry {
            ro,
            anchors,
            read_ids,
            out,
            fin,
        });
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
        let miss = |why: String| lock(&self.memo.misses).push((id, k, why));
        let e = {
            let prev = self.memo.prev.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(v) = prev.get(&id) else {
                drop(prev);
                miss("no entry".into());
                return false;
            };
            // The last entry at or before the round (the final phase's only
            // in the final phase).
            let e = if k == FINAL {
                v.last().map(|x| &x.1)
            } else {
                v.iter().rev().find(|x| x.0 <= k).map(|x| &x.1)
            };
            let Some(e) = e else {
                drop(prev);
                miss("no entry at or before the round".into());
                return false;
            };
            e.clone()
        };
        let r = self.rec(s);
        {
            let d = r.data();
            if d.op != e.ro.op || d.args != e.ro.args {
                let which = if d.op != e.ro.op {
                    "its operation".to_string()
                } else {
                    let i = d.args.iter().zip(e.ro.args.iter()).position(|(a, b)| a != b);
                    match i {
                        Some(i) => format!(
                            "arg {i}: {} -> {}",
                            self.values.render(e.ro.args[i]),
                            self.values.render(d.args[i])
                        ),
                        None => format!("its arg count {} -> {}", e.ro.args.len(), d.args.len()),
                    }
                };
                drop(d);
                miss(format!("text changed ({which})"));
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
                    _ => {
                        miss(format!("anchor node {id} gone"));
                        return false;
                    }
                },
            }
        }
        let mut ro = e.ro.clone();
        // Its reads to the nodes' current slots and generations (a node
        // gone: no offer).
        let mut ids = e.read_ids.iter();
        let mut gone = false;
        for p in &mut ro.log {
            visit_slots(p, &mut |x, gn| {
                let Some(&id) = ids.next() else {
                    gone = true;
                    return;
                };
                if id == u64::MAX {
                    return;
                }
                match self.slot_of(id) {
                    Some(y) if self.rec(y).st() != super::S_FREE => {
                        *x = y;
                        if let Some(g) = gn {
                            *g = self.rec(y).gn.load(Ordering::Acquire);
                        }
                    }
                    _ => gone = true,
                }
            });
        }
        if gone {
            miss("a node its reads name is gone".into());
            return false;
        }
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
    /// whether every round of the last evaluation would see the same
    /// change, so its round sequence stands and the state reached is the
    /// cold one. By value: the nodes the quick path committed are compared
    /// with their last entries; every node that committed, and every reader
    /// (in any round) of a value that changed, must have committed the same
    /// in every round of the last evaluation (round-invariant: what it read
    /// did not depend on the round), read nothing new that is not, and
    /// publish nothing changed. A reader of an unchanged value is not
    /// looked at (the cone stops there, as the quick path's runs did).
    pub(crate) fn quick_holds(&self) -> Result<(), String> {
        let cur = lock(&self.memo.cur);
        let prev = self.memo.prev.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        let ix = lock(&self.memo.ix);
        let name = |id: u64| -> String {
            match self.slot_of(id) {
                Some(x) => {
                    let d = self.rec(x).data();
                    let a: Vec<String> = d.args.iter().map(|&v| self.values.render(v)).collect();
                    format!("node {id} ({})", a.join(" "))
                }
                None => format!("node {id} (gone)"),
            }
        };
        let fname = |f: crate::values::Field| self.values.render(f.0);
        let moving: FxHashSet<crate::values::Field> = lock(&self.memo.prev_changes)
            .iter()
            .flat_map(|c| c.fields.iter().copied())
            .collect();
        // Round-invariant in the last evaluation: committed the same in
        // every round, reading no loop-back that moved (one entry alone
        // does not say so: a round may have started from values other than
        // the initial ones). (New: judged by its own reads, below.)
        let invariant = |id: u64| {
            prev.get(&id).is_none_or(|v| {
                v.iter().all(|(_, e)| {
                    e.same(&v[0].1) && (e.fin || loop_fields(&e.ro.log).iter().all(|f| !moving.contains(f)))
                })
            })
        };
        let mut seen: FxHashSet<u64> = FxHashSet::default();
        let mut work: Vec<(Key, u64)> = Vec::new();
        for (id, v) in cur.iter() {
            let Some((_, new)) = v.last() else { continue };
            seen.insert(*id);
            let last = prev.get(id).and_then(|o| o.last()).map(|x| &x.1);
            let x = self.slot_of(*id);
            match last {
                None => {
                    // New (inserted, or made by a run): no scope effect,
                    // claim or unfolding of its own.
                    let Some(x) = x else {
                        return Err(format!("{} is new and gone", name(*id)));
                    };
                    let plain = {
                        let d = self.rec(x).data();
                        d.stat.effect == crate::api::Effect::None && d.stat.claim == super::ClaimS::None
                    };
                    if !plain || new.ro.claim.is_some() || !new.ro.kids.is_empty() || !new.ro.conts.is_empty() {
                        return Err(format!("{} is new with a scope effect, claim or unfolding", name(*id)));
                    }
                }
                Some(last) => {
                    // (A changed text is judged by its new reads alone.)
                    let same_text = last.ro.op == new.ro.op && last.ro.args == new.ro.args;
                    if same_text && !invariant(*id) {
                        return Err(format!(
                            "{} sees a changed input and committed differently in a later round",
                            name(*id)
                        ));
                    }
                    if new.ro.effect != last.ro.effect
                        || new.ro.claim != last.ro.claim
                        || new.ro.kids.len() != last.ro.kids.len()
                        || new.ro.conts.len() != last.ro.conts.len()
                        || new.ro.kids.iter().zip(&last.ro.kids).any(|(a, b)| a.id() != b.id())
                        || new.ro.conts.iter().zip(&last.ro.conts).any(|(a, b)| a.id() != b.id())
                    {
                        return Err(format!("{}'s scope effect, claim or unfolding changed", name(*id)));
                    }
                }
            }
            if !new.fin {
                let old_pubs = last.map_or(&[][..], |l| &l.ro.pubs[..]);
                // A publisher new or re-texted: its final publication may
                // equal the old one's while its earlier rounds' do not.
                let same_text = last.is_some_and(|l| l.ro.op == new.ro.op && l.ro.args == new.ro.args);
                if !same_text && (!new.ro.pubs.is_empty() || !old_pubs.is_empty() || ix.publishers.contains(id)) {
                    return Err(format!("{} publishes a loop-back and is new or re-texted", name(*id)));
                }
                if new.ro.pubs != old_pubs {
                    let f = new
                        .ro
                        .pubs
                        .iter()
                        .chain(old_pubs)
                        .find(|p| !(new.ro.pubs.contains(p) && old_pubs.contains(p)))
                        .map_or(String::new(), |p| fname(p.0));
                    return Err(format!("{} changed its loop-back publication of {f}", name(*id)));
                }
                // A loop-back that moved between rounds: its value differs
                // by round.
                if let Some(f) = loop_fields(&new.ro.log).into_iter().find(|f| moving.contains(f)) {
                    return Err(format!(
                        "{} reads the loop-back {}, which moved between rounds",
                        name(*id),
                        fname(f)
                    ));
                }
            }
            // What it reads anew must not depend on the round.
            // (A changed text's old reads say nothing of its new ones.)
            let old_ids: FxHashSet<u64> = last
                .filter(|l| l.ro.op == new.ro.op && l.ro.args == new.ro.args)
                .map_or_else(FxHashSet::default, |l| l.read_ids.iter().copied().collect());
            for &r in &new.read_ids {
                if r != u64::MAX && r != *id && !old_ids.contains(&r) && !cur.contains_key(&r) && !invariant(r) {
                    return Err(format!(
                        "{} now reads {}, which committed differently by round",
                        name(*id),
                        name(r)
                    ));
                }
            }
            // What changed, to its readers.
            let (l_out, l_kind, l_kinds, l_contribs, l_env, l_defs) = match last {
                Some(l) => (
                    Some(l.out),
                    l.ro.kind,
                    &l.ro.kinds[..],
                    &l.ro.contribs[..],
                    &l.ro.env[..],
                    &l.ro.defs[..],
                ),
                None => (None, None, &[][..], &[][..], &[][..], &[][..]),
            };
            if (Some(new.out) != l_out || new.ro.kind != l_kind || new.ro.kinds != l_kinds)
                && let Some(x) = x
            {
                work.push((Key::N(x), *id));
            }
            for f in changed_fields(&new.ro.contribs, l_contribs) {
                work.push((Key::F(f), *id));
            }
            for f in changed_fields(&new.ro.env, l_env) {
                work.push((Key::F(f), *id));
            }
            let mut df: Vec<crate::values::Field> = new
                .ro
                .defs
                .iter()
                .filter(|d| !l_defs.contains(d))
                .chain(l_defs.iter().filter(|d| !new.ro.defs.contains(d)))
                .map(|d| d.f)
                .collect();
            df.sort_unstable();
            df.dedup();
            for f in df {
                match x {
                    Some(x) => {
                        let mut w = Vec::new();
                        self.def_window(&ix, x, f, &mut w);
                        work.extend(w.into_iter().map(|k| (k, *id)));
                        work.push((Key::N(x), *id));
                    }
                    None => work.push((Key::F(f), *id)),
                }
            }
        }
        // Inserts and removals: under a parent whose children a read
        // walked, they shift what it saw; a removed node with a scope effect
        // or a claim, or that published, is not judged; otherwise the
        // readers of what it defined, contributed or was named by are.
        for p in lock(&self.memo.insert_parents).iter() {
            if ix.sib_parents.contains(p) {
                return Err("an insert under a parent whose children a read walked".into());
            }
        }
        for &(id, x, parent, bad) in lock(&self.memo.removed).iter() {
            if bad || ix.publishers.contains(&id) || ix.sib_parents.contains(&parent) {
                return Err(format!(
                    "removed {} had a scope effect, claim or publication, or its parent's children were walked",
                    name(id)
                ));
            }
            for &f in ix.contribs_of.get(&id).into_iter().flatten() {
                work.push((Key::F(f), id));
            }
            for &f in ix.defs_of.get(&id).into_iter().flatten() {
                work.push((Key::F2(f), id));
            }
            work.push((Key::N(x), id));
            seen.insert(id);
        }
        // The readers of a changed value (not further: what they changed
        // in turn, the quick path committed, and was compared above).
        let mut done_f: FxHashSet<crate::values::Field> = FxHashSet::default();
        let mut done_f2: FxHashSet<crate::values::Field> = FxHashSet::default();
        let mut done_n: FxHashSet<Slot> = FxHashSet::default();
        let check = |w: u64, why: &dyn Fn() -> String, seen: &mut FxHashSet<u64>| -> Result<(), String> {
            if !seen.insert(w) || cur.contains_key(&w) {
                return Ok(());
            }
            if !invariant(w) {
                return Err(format!(
                    "{} reads {} and committed differently in a later round",
                    name(w),
                    why()
                ));
            }
            Ok(())
        };
        while let Some((k, src)) = work.pop() {
            let (readers, why): (Option<&Vec<u64>>, String) = match k {
                Key::F(f) => {
                    if !done_f.insert(f) {
                        continue;
                    }
                    let why = format!("{} (changed by {})", fname(f), name(src));
                    for &ws in ix.by_field_use.get(&f).into_iter().flatten() {
                        if self.rec(ws).st() != super::S_FREE {
                            check(self.node_id(ws).0, &|| why.clone(), &mut seen)?;
                        }
                    }
                    (ix.by_field.get(&f), why)
                }
                Key::F2(f) => {
                    if done_f.contains(&f) || !done_f2.insert(f) {
                        continue;
                    }
                    (ix.by_field.get(&f), format!("{} (changed by {})", fname(f), name(src)))
                }
                Key::R(w) => {
                    check(w, &|| format!("a definition changed by {}", name(src)), &mut seen)?;
                    continue;
                }
                Key::N(x) => {
                    if !done_n.insert(x) {
                        continue;
                    }
                    (ix.by_slot.get(&x), format!("{}, changed", name(src)))
                }
            };
            for &w in readers.into_iter().flatten() {
                check(w, &|| why.clone(), &mut seen)?;
            }
        }
        // The round count: a counted round stands while a reader of the
        // change it followed is out of reach (it read the same in every
        // round). (An uncounted one stays so: no node in reach reads a
        // loop-back anew, and one that read a moving one is not
        // round-invariant.)
        for c in lock(&self.memo.prev_changes).iter() {
            if c.counted && !c.readers.iter().any(|r| !seen.contains(r) && !ix.uncertain.contains(r)) {
                return Err("a counted round's readers are all in reach".into());
            }
        }
        Ok(())
    }

    /// The readers by use of `f` a new definition of it by `x` can reach:
    /// those after it, up to the next definition that shadows it for them
    /// (the next in its frame, or the next global for a global one).
    fn def_window(&self, ix: &Index, x: Slot, f: crate::values::Field, work: &mut Vec<Key>) {
        // Readers by sequence, range or stream: all of them.
        work.push(Key::F2(f));
        let lo = self.rec(x).lab();
        let u = self.unit_of(x);
        let hi = {
            let sh = super::rlock(self.shard(u, f));
            sh.get(&(u, f)).and_then(|idx| {
                let e = idx.defs.iter().find(|e| e.node == x)?;
                let list: &[Slot] = if e.loc == crate::api::Locality::Global {
                    &idx.globals
                } else {
                    idx.local.get(&e.fk).map_or(&[][..], Vec::as_slice)
                };
                // (By label: the first after it.)
                let i = list.partition_point(|&y| self.rec(y).lab() <= lo);
                list.get(i).map(|&y| self.rec(y).lab())
            })
        };
        for &ws in ix.by_field_use.get(&f).into_iter().flatten() {
            let r = self.rec(ws);
            if r.st() == super::S_FREE {
                continue;
            }
            let l = r.lab();
            if l > lo && hi.is_none_or(|h| l < h) && self.unit_of(ws) == u {
                work.push(Key::R(self.node_id(ws).0));
            }
        }
    }

    /// A read, for a miss's reason: its kind, the field and the node it
    /// names.
    fn describe_read(&self, p: &Pred) -> String {
        let f = |f: crate::values::Field| self.values.render(f.0);
        let n = |x: Slot| {
            if x == super::NIL || self.rec(x).st() == super::S_FREE {
                "-".to_string()
            } else {
                self.node_id(x).0.to_string()
            }
        };
        match p {
            Pred::Def { f: g, ans, .. } => match ans {
                super::DAns::Site { node, .. } => format!("read value changed: {} (from node {})", f(*g), n(*node)),
                super::DAns::Loop(_) => format!("read value changed: loop-back {}", f(*g)),
                super::DAns::Env(_) => format!("read value changed: environment {}", f(*g)),
                _ => format!("read value changed: {}", f(*g)),
            },
            Pred::Range { f: g, .. } => format!("range changed: {}", f(*g)),
            Pred::Out { node, .. } => format!("output changed: node {}", n(*node)),
            Pred::At { node, .. } => format!("position changed: node {}", n(*node)),
            Pred::App { node, .. } => format!("application changed: node {}", n(*node)),
            Pred::Env { f: g, .. } => format!("environment changed: {}", f(*g)),
            Pred::IsLoop { f: g, .. } => format!("loop-back declaration changed: {}", f(*g)),
            Pred::Kind { opener, .. } => format!("kind changed: opener {}", n(*opener)),
            Pred::Sib { .. } => "following sibling changed".into(),
            Pred::PrevSib { .. } => "preceding siblings changed".into(),
            Pred::Claimed { .. } => "claimed siblings changed".into(),
            Pred::Stream { .. } => "stream changed".into(),
            #[allow(unreachable_patterns)]
            _ => "a read changed".into(),
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
                let k = self.memo.round.load(Ordering::Acquire);
                let why = o
                    .log
                    .iter()
                    .find(|p| matches!(self.check_log(s, std::slice::from_ref(*p)), super::run::Check::Broke(_)))
                    .map_or_else(|| "its reads changed meanwhile".to_string(), |p| self.describe_read(p));
                lock(&self.memo.misses).push((self.node_id(s).0, k, why));
            }
            self.not_committed(s, w);
        }
    }
}

/// What a read depends on: a field, or a node it names; or a reader
/// reached directly.
enum Key {
    F(crate::values::Field),
    /// By field, its readers by sequence, range or stream only.
    F2(crate::values::Field),
    N(Slot),
    R(u64),
}

/// Calls `k` with every field and node `p` names.
fn read_keys(p: &Pred, k: &mut impl FnMut(Key)) {
    match p {
        Pred::Def {
            f, mode, ans, sites, ..
        } => {
            if *mode != super::DMode::Use {
                k(Key::F(*f));
            }
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

/// Calls `f` with every slot a read names (the reader's own position
/// too), and its generation where the read keeps one.
fn visit_slots(p: &mut Pred, f: &mut impl FnMut(&mut Slot, Option<&mut u32>)) {
    match p {
        Pred::Def { at, ans, sites, .. } => {
            f(at, None);
            if let super::DAns::Site { node, gn, .. } = ans {
                f(node, Some(gn));
            }
            for (x, g) in sites.iter_mut() {
                f(x, Some(g));
            }
        }
        Pred::At { node, gn } | Pred::Out { node, gn, .. } | Pred::App { node, gn, .. } => f(node, Some(gn)),
        Pred::Range { at, last, .. } => {
            f(at, None);
            f(last, None);
        }
        Pred::Kind { at, opener, .. } => {
            f(at, None);
            f(opener, None);
        }
        Pred::Sib {
            ans: Some((x, g, _)), ..
        } => f(x, Some(g)),
        Pred::PrevSib { seen, ans } => {
            for (x, g, _) in seen.iter_mut() {
                f(x, Some(g));
            }
            if let Some(x) = ans {
                f(x, None);
            }
        }
        Pred::Claimed { list } => {
            for (x, g, _) in list.iter_mut() {
                f(x, Some(g));
            }
        }
        Pred::Stream { seen, reads, .. } => {
            for (x, g, _) in seen.iter_mut() {
                f(x, Some(g));
            }
            for (x, _) in reads.iter_mut() {
                f(x, None);
            }
        }
        _ => {}
    }
}
