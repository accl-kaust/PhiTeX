//! The build's top level as a fold of steps, with each slot's
//! definitions and their readers (`DESIGN.md` 7.17.1, 7.17.3, 7.17.9).
//!
//! A *step* is a call of the top level, from one clean point to the next.
//! A step's net writes are its *definitions*: each slot keeps its
//! definitions in program order, the value of each in the step's record.
//! A step's *outside reads* are the reads its subtree made of a slot
//! whose definition then came before the step began; the slot's
//! *readers* are the steps that read it so. The state at a point is
//! stored nowhere: it is, slot by slot, the definition that reaches the
//! point ([`Fold::reaching`]). A rebuild runs a step again in its place
//! ([`Fold::rerun`]): its new definitions and reads replace the old ones,
//! and a definition whose value changed makes its old readers dirty, up
//! to the slot's next definition ([`Fold::readers_between`]).
//!
//! Steps are named by ids that never change, and ordered by keys: a
//! rebuild that ends a step elsewhere splices steps in between two old
//! ones with keys between theirs. An index entry names the step and the
//! run of it that made the entry, so a step run again leaves its old
//! entries dead in place.

use alloc::vec::Vec;

use crate::hash::hash64;
use crate::machine::Machine;
use crate::runtime::RecId;
use crate::table::{ByHash, Table};

/// A step's id: its index in [`Fold::steps`], never reused.
pub type StepId = u32;

/// The gap between the keys of consecutive steps of a cold build (room
/// for a thousand new steps in a run, a thousand keystrokes' runs at one
/// place, before [`Fold::renumber`]; 2^31 steps before a key overflows).
pub const KEY_GAP: u64 = 1 << 32;

/// A step of the fold.
#[derive(Clone, Debug)]
pub struct Step<A> {
    /// Its place in program order.
    pub key: u64,
    /// Its records, the top level's calls it made, in order.
    pub recs: Vec<RecId>,
    /// The slots it read from outside it, each once, in the order read.
    pub reads: Vec<A>,
    /// Which run of the step made its entries (a run again bumps it).
    pub run: u32,
    /// Which run made its reader entries: a run that read what the run
    /// before it read, in that order, keeps them, and this
    /// ([`Fold::close`]).
    pub rrun: u32,
    /// Whether it is in the fold (a rebuild removes the steps it passes
    /// over when it ends a step elsewhere).
    pub live: bool,
}

/// An entry of an index: step `step`'s run `run`; for a definition, the
/// write at `ix` of the step's record `rec`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    step: StepId,
    run: u32,
    key: u64,
    rec: RecId,
    ix: u32,
}

/// A definition found: the record and the index of the write in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Def {
    pub step: StepId,
    pub key: u64,
    pub rec: RecId,
    pub ix: u32,
}

/// The fold and its indexes.
pub struct Fold<M: Machine> {
    pub steps: Vec<Step<M::Addr>>,
    /// The live steps in program order.
    pub order: Vec<StepId>,
    /// Each slot's definitions and readers, by the address's hash, in
    /// key order.
    defs: Table<ByHash<M::Addr>, Vec<Entry>>,
    readers: Table<ByHash<M::Addr>, Vec<Entry>>,
    /// How many times the keys were made again ([`Fold::renumber`]).
    pub renumbered: u32,
}

impl<M: Machine> Default for Fold<M> {
    fn default() -> Self {
        Fold {
            steps: Vec::new(),
            order: Vec::new(),
            defs: Table::new(),
            readers: Table::new(),
            renumbered: 0,
        }
    }
}

impl<M: Machine> Fold<M> {
    fn live(&self, e: &Entry) -> bool {
        self.steps
            .get(e.step as usize)
            .is_some_and(|s| s.live && s.run == e.run)
    }

    /// Whether reader entry `e` is its step's (made by the run that made
    /// the step's reader entries).
    fn live_reader(&self, e: &Entry) -> bool {
        self.steps
            .get(e.step as usize)
            .is_some_and(|s| s.live && s.rrun == e.run)
    }

    /// A new step at the end of the fold.
    pub(crate) fn push(&mut self) -> StepId {
        let key = self
            .order
            .last()
            .map_or(KEY_GAP, |&l| self.steps[l as usize].key + KEY_GAP);
        self.insert_at(self.order.len(), key)
    }

    /// A new step at `pos` of the order, with key `key`.
    pub(crate) fn insert_at(&mut self, pos: usize, key: u64) -> StepId {
        let id = StepId::try_from(self.steps.len()).expect("fewer than 2^32 steps");
        self.steps.push(Step {
            key,
            recs: Vec::new(),
            reads: Vec::new(),
            run: 0,
            rrun: 0,
            live: true,
        });
        self.order.insert(pos, id);
        id
    }

    /// A new step right after live step `id` (DESIGN 7.17.3 item 4: a
    /// step that ended elsewhere runs on), its key a sixty-fourth of the
    /// gap to the next step's away from one end: from the next step's
    /// back toward `id`'s for the first new step after a step run again
    /// (`near_next`), from `id`'s toward the next step's for a later one
    /// of the same run.
    ///
    /// A step run again that ends elsewhere at every keystroke (the
    /// `.aux` read again at the job's end, after a heading changed) puts
    /// its first new step before the ones the last keystroke put there,
    /// which this run passes over: put near `id`, the gap after it shrank
    /// sixty-four times a keystroke and the fold was numbered again every
    /// third one (half a heading keystroke's time in the browser); put
    /// near the next step, it shrinks by a sixty-fourth. A run of new
    /// steps, each after the last, takes a sixty-fourth of what is left
    /// each time. With no key left between the two, the fold's keys are
    /// made again ([`Fold::renumber`]).
    ///
    /// `None`: `id` is not live.
    pub fn insert_after(&mut self, id: StepId, near_next: bool) -> Option<StepId> {
        let mut pos = self.position(id)?;
        let next_key = |f: &Self, pos: usize| {
            let key = f.steps[f.order[pos] as usize].key;
            let next = f
                .order
                .get(pos + 1)
                .map_or(key + 2 * KEY_GAP, |&n| f.steps[n as usize].key);
            (key, next)
        };
        let (mut key, mut next) = next_key(self, pos);
        if next - key < 2 {
            self.renumber();
            pos = self.position(id)?;
            (key, next) = next_key(self, pos);
        }
        let d = ((next - key) / 64).max(1);
        Some(self.insert_at(pos + 1, if near_next { next - d } else { key + d }))
    }

    /// Give the live steps keys [`KEY_GAP`] apart again, in their order,
    /// and the index entries theirs (the dead ones go).
    pub fn renumber(&mut self) {
        self.renumbered += 1;
        for (i, &s) in self.order.iter().enumerate() {
            self.steps[s as usize].key = (i as u64 + 1) * KEY_GAP;
        }
        let steps = &self.steps;
        let fix = |v: &mut Vec<Entry>, reads: bool| {
            v.retain(|e| {
                steps
                    .get(e.step as usize)
                    .is_some_and(|s| s.live && e.run == if reads { s.rrun } else { s.run })
            });
            for e in v.iter_mut() {
                e.key = steps[e.step as usize].key;
            }
        };
        self.defs.values_mut().for_each(|v| fix(v, false));
        self.readers.values_mut().for_each(|v| fix(v, true));
    }

    /// Remove step `id` from the fold, and its entries: as a reader of
    /// the slots it read from outside it, and as the definition of each
    /// slot in `written` (its records' writes). Left in their lists, dead,
    /// a removed step's entries went only when the keys were made again
    /// ([`Fold::renumber`]): every third keystroke in a heading did that
    /// once, and with it rare, the lists grew at each one (four steps
    /// passed over), and each keystroke took longer than the last.
    pub fn remove<'a>(&mut self, id: StepId, written: impl IntoIterator<Item = &'a M::Addr>)
    where
        M::Addr: 'a,
    {
        if let Some(pos) = self.position(id) {
            self.order.remove(pos);
        } else {
            self.order.retain(|&s| s != id);
        }
        let s = &mut self.steps[id as usize];
        s.live = false;
        let key = s.key;
        for a in &self.steps[id as usize].reads {
            if let Some(v) = self.readers.get_mut_by(hash64(a), |k| k.0 == *a) {
                Self::drop_step(v, id, key);
            }
        }
        for a in written {
            if let Some(v) = self.defs.get_mut_by(hash64(a), |k| k.0 == *a) {
                Self::drop_step(v, id, key);
            }
        }
    }

    /// Close step `id`: its outside reads (`reads`, each with its
    /// address's hash) and its records' writes (`writes` gives each
    /// record's written addresses) become index entries.
    ///
    /// A step run again replaces its last run's entries where they are
    /// (the step's entry of a slot is at its key): an entry this run makes
    /// again takes the old one's place, and the old run's entries this run
    /// does not make are removed. So a slot's lists hold only live entries
    /// but those of removed steps, and closing a run costs its own reads
    /// and writes, each a search in its slot's list, with nothing moved
    /// unless the run reads or defines a slot its last run did not: a slot
    /// every step reads (a catcode, `\baselineskip`) has a list as long as
    /// the fold, and an insertion moves its tail.
    pub(crate) fn close(
        &mut self,
        id: StepId,
        recs: Vec<RecId>,
        reads: Vec<(u64, M::Addr)>,
        writes: impl Fn(RecId) -> Vec<M::Addr>,
    ) {
        // (this run's entries replace the last run's, which stayed live
        // while it ran: a rebuild compares against them)
        let (key, run, old_recs, old_reads) = {
            let s = &mut self.steps[id as usize];
            s.run += 1;
            (
                s.key,
                s.run,
                core::mem::take(&mut s.recs),
                core::mem::take(&mut s.reads),
            )
        };
        // (a run that read what the run before it read, in that order,
        // keeps that run's reader entries: a slot every step reads has a
        // list as long as the fold, and a search in it per read is most of
        // a rerun step's close)
        let same = reads.len() == old_reads.len()
            && reads.iter().zip(&old_reads).all(|((_, a), b)| a == b);
        let addrs = if same {
            old_reads
        } else {
            let rrun = {
                let s = &mut self.steps[id as usize];
                s.rrun += 1;
                s.rrun
            };
            let mut addrs = Vec::with_capacity(reads.len());
            for (h, a) in reads {
                let e = Entry {
                    step: id,
                    run: rrun,
                    key,
                    rec: 0,
                    ix: 0,
                };
                Self::put(
                    self.readers
                        .entry_by(h, |k| k.0 == a, || ByHash(a.clone()), Vec::new()),
                    e,
                );
                addrs.push(a);
            }
            for a in &old_reads {
                if let Some(v) = self.readers.get_mut_by(hash64(a), |k| k.0 == *a) {
                    Self::drop_stale(v, id, key, rrun);
                }
            }
            addrs
        };
        for &rec in &recs {
            for (ix, a) in writes(rec).iter().enumerate() {
                let e = Entry {
                    step: id,
                    run,
                    key,
                    rec,
                    ix: u32::try_from(ix).expect("fewer than 2^32 writes"),
                };
                // (a later record of the step defines the slot again: the
                // step's definition is its last)
                Self::put(
                    self.defs
                        .entry_by(hash64(a), |k| k.0 == *a, || ByHash(a.clone()), Vec::new()),
                    e,
                );
            }
        }
        for &rec in &old_recs {
            for a in writes(rec) {
                if let Some(v) = self.defs.get_mut_by(hash64(&a), |k| k.0 == a) {
                    Self::drop_stale(v, id, key, run);
                }
            }
        }
        let s = &mut self.steps[id as usize];
        s.recs = recs;
        s.reads = addrs;
    }

    /// Put `e` in `v`, sorted by key: over its step's entry at its key (the
    /// step's last run's, or an earlier record's of this run), else
    /// inserted after the entries at its key.
    fn put(v: &mut Vec<Entry>, e: Entry) {
        let at = v.partition_point(|x| x.key < e.key);
        let same = v[at..].iter().take_while(|x| x.key == e.key).count();
        match v[at..at + same].iter_mut().find(|x| x.step == e.step) {
            Some(x) => *x = e,
            None => v.insert(at + same, e),
        }
    }

    /// Remove step `step`'s entry at `key` from `v` if a run before `run`
    /// made it.
    fn drop_stale(v: &mut Vec<Entry>, step: StepId, key: u64, run: u32) {
        let at = v.partition_point(|x| x.key < key);
        if let Some(i) = v[at..]
            .iter()
            .take_while(|x| x.key == key)
            .position(|x| x.step == step && x.run != run)
        {
            v.remove(at + i);
        }
    }

    /// Remove step `step`'s entries at `key` from `v`.
    fn drop_step(v: &mut Vec<Entry>, step: StepId, key: u64) {
        let at = v.partition_point(|x| x.key < key);
        let n = v[at..].iter().take_while(|x| x.key == key).count();
        let mut i = at;
        for _ in 0..n {
            if v[i].step == step {
                v.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// The definition of `a` that reaches key `key`: the last live one
    /// before it (`None`: the slot's value before the build defined it).
    #[must_use]
    pub fn reaching(&self, a: &M::Addr, key: u64) -> Option<Def> {
        let v = self.defs.get_by(hash64(a), |k| k.0 == *a)?;
        let at = v.partition_point(|x| x.key < key);
        v[..at].iter().rev().find(|e| self.live(e)).map(Self::def)
    }

    /// The last live definition of `a`.
    #[must_use]
    pub fn latest(&self, a: &M::Addr) -> Option<Def> {
        let v = self.defs.get_by(hash64(a), |k| k.0 == *a)?;
        v.iter().rev().find(|e| self.live(e)).map(Self::def)
    }

    /// The first live definition of `a` after key `key`.
    #[must_use]
    pub fn next_after(&self, a: &M::Addr, key: u64) -> Option<Def> {
        let v = self.defs.get_by(hash64(a), |k| k.0 == *a)?;
        let at = v.partition_point(|x| x.key <= key);
        v[at..].iter().find(|e| self.live(e)).map(Self::def)
    }

    fn def(e: &Entry) -> Def {
        Def {
            step: e.step,
            key: e.key,
            rec: e.rec,
            ix: e.ix,
        }
    }

    /// The live steps that read `a` from outside them with keys in
    /// `(lo, hi]` (`hi` `None`: to the end).
    #[must_use]
    pub fn readers_between(&self, a: &M::Addr, lo: u64, hi: Option<u64>) -> Vec<StepId> {
        let Some(v) = self.readers.get_by(hash64(a), |k| k.0 == *a) else {
            return Vec::new();
        };
        let from = v.partition_point(|x| x.key <= lo);
        v[from..]
            .iter()
            .take_while(|e| hi.is_none_or(|h| e.key <= h))
            .filter(|e| self.live_reader(e))
            .map(|e| e.step)
            .collect()
    }

    /// Step `id`'s position in [`Fold::order`].
    #[must_use]
    pub fn position(&self, id: StepId) -> Option<usize> {
        let key = self.steps.get(id as usize)?.key;
        let at = self
            .order
            .partition_point(|&s| self.steps[s as usize].key < key);
        (self.order.get(at) == Some(&id)).then_some(at)
    }

    /// The records of the live steps, for the collector's roots.
    pub fn roots(&self) -> impl Iterator<Item = RecId> + '_ {
        self.order
            .iter()
            .flat_map(|&s| self.steps[s as usize].recs.iter().copied())
    }
}
