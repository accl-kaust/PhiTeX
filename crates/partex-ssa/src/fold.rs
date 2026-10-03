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
//! ones with keys between theirs. An index entry names the step, never
//! its key: a slot's definitions are the live steps' last runs' writes of
//! it, its readers the live steps whose last run read it, each list in
//! the order of the steps' keys.

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
    /// Whether it is in the fold (a rebuild removes the steps it passes
    /// over when it ends a step elsewhere).
    pub live: bool,
}

/// An entry of a slot's definitions: step `step`'s write at `ix` of its
/// record `rec`, made by the step's last run (a run's close puts its own
/// over them and removes the rest), its key the step's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    step: StepId,
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
    /// Each step's key, by id, as [`Step::key`] holds it: the index's
    /// searches read them here, eight bytes a step where a step is 72.
    keys: Vec<u64>,
    /// The live steps in program order.
    pub order: Vec<StepId>,
    /// Each slot's definitions, by the address's hash, in the order of
    /// the steps' keys.
    defs: Table<ByHash<M::Addr>, Vec<Entry>>,
    /// Each slot's readers, by the address's hash: the live steps whose
    /// last run read it from outside them, in the order of their keys. An
    /// entry is the step's id alone (4 bytes where a definition's is 24: a
    /// slot every step reads has a list as long as the fold), its key the
    /// step's, so keys made again leave the lists as they are.
    readers: Table<ByHash<M::Addr>, Vec<StepId>>,
    /// How many times the keys were made again ([`Fold::renumber`]).
    pub renumbered: u32,
}

impl<M: Machine> Default for Fold<M> {
    fn default() -> Self {
        Fold {
            steps: Vec::new(),
            keys: Vec::new(),
            order: Vec::new(),
            defs: Table::new(),
            readers: Table::new(),
            renumbered: 0,
        }
    }
}

impl<M: Machine> Fold<M> {
    fn live(&self, e: &Entry) -> bool {
        self.steps.get(e.step as usize).is_some_and(|s| s.live)
    }

    /// Step `s`'s key.
    fn key_of(&self, s: StepId) -> u64 {
        self.keys[s as usize]
    }

    /// What the fold holds, roughly, in bytes by part (a report:
    /// `PARTEX_SSA_MEM`): the steps, their reads, and the reader and
    /// definition entries.
    #[must_use]
    pub fn mem_report(&self) -> alloc::string::String {
        use core::mem::size_of;
        fn entries<K: crate::table::TKey, E>(t: &Table<K, Vec<E>>) -> (usize, usize, usize) {
            let (mut n, mut cap) = (0, 0);
            for (_, v) in t.iter() {
                n += v.len();
                cap += v.capacity();
            }
            (t.len(), n, cap)
        }
        let (rs, rn, rc) = entries(&self.readers);
        // (the readers' lists by length: of a slot most steps read, a
        // list as long as the fold)
        let mut hist = [(0usize, 0usize); 6];
        for (_, v) in self.readers.iter() {
            let k = match v.len() {
                0..=4 => 0,
                5..=64 => 1,
                65..=1024 => 2,
                1025..=8192 => 3,
                8193..=32768 => 4,
                _ => 5,
            };
            hist[k].0 += 1;
            hist[k].1 += v.len();
        }
        let (ds, dn, dc) = entries(&self.defs);
        let (mut sr, mut src, mut recs) = (0, 0, 0);
        for s in &self.steps {
            sr += s.reads.len();
            src += s.reads.capacity();
            recs += s.recs.capacity();
        }
        let (a, e, r) = (
            size_of::<M::Addr>(),
            size_of::<Entry>(),
            size_of::<StepId>(),
        );
        alloc::format!(
            "fold: {} steps ({} B each, {} MB); their reads {sr} (capacity {src}, {} MB), records {recs}; readers: {rs} slots, {rn} entries (capacity {rc}, {r} B: {} MB), by a list's length (slots, entries) <=4 {:?}, <=64 {:?}, <=1K {:?}, <=8K {:?}, <=32K {:?}, more {:?}; definitions: {ds} slots, {dn} entries (capacity {dc}, {e} B: {} MB)",
            self.steps.len(),
            size_of::<Step<M::Addr>>(),
            (self.steps.capacity() * size_of::<Step<M::Addr>>()) >> 20,
            (src * a) >> 20,
            (rc * r) >> 20,
            hist[0],
            hist[1],
            hist[2],
            hist[3],
            hist[4],
            hist[5],
            (dc * e) >> 20,
        )
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
            live: true,
        });
        self.keys.push(key);
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

    /// Give the live steps keys [`KEY_GAP`] apart again, in their order.
    /// The index's lists, in the steps' order and naming no key, stay in
    /// order; an entry of a removed step, whose key stays as it was, would
    /// not, and goes (there is none: [`Fold::remove`] takes them).
    pub fn renumber(&mut self) {
        self.renumbered += 1;
        for (i, &s) in self.order.iter().enumerate() {
            let key = (i as u64 + 1) * KEY_GAP;
            self.steps[s as usize].key = key;
            self.keys[s as usize] = key;
        }
        let steps = &self.steps;
        let live = |s: StepId| steps.get(s as usize).is_some_and(|s| s.live);
        self.defs
            .values_mut()
            .for_each(|v| v.retain(|e| live(e.step)));
        self.readers
            .values_mut()
            .for_each(|v| v.retain(|&s| live(s)));
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
        let key = self.keys[id as usize];
        for a in &self.steps[id as usize].reads {
            if let Some(v) = self.readers.get_mut_by(hash64(a), |k| k.0 == *a) {
                drop_reader(&self.keys, v, id, key);
            }
        }
        for a in written {
            if let Some(v) = self.defs.get_mut_by(hash64(a), |k| k.0 == *a) {
                let at = v.partition_point(|x| self.keys[x.step as usize] < key);
                if v.get(at).is_some_and(|x| x.step == id) {
                    v.remove(at);
                }
            }
        }
        self.steps[id as usize].live = false;
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
        let (key, old_recs, old_reads) = {
            let s = &mut self.steps[id as usize];
            s.run += 1;
            (
                s.key,
                core::mem::take(&mut s.recs),
                core::mem::take(&mut s.reads),
            )
        };
        // (a run that read what the run before it read, in that order,
        // keeps the step's reader entries: a slot every step reads has a
        // list as long as the fold, and a search in it per read is most of
        // a rerun step's close; else the step goes into the lists of the
        // slots it reads that the last run did not, and out of those the
        // last run read that it does not)
        let same = reads.len() == old_reads.len()
            && reads.iter().zip(&old_reads).all(|((_, a), b)| a == b);
        let addrs = if same {
            old_reads
        } else {
            let Fold { keys, readers, .. } = self;
            if old_reads.is_empty() {
                for (h, a) in &reads {
                    let v = readers.entry_by(*h, |k| k.0 == *a, || ByHash(a.clone()), Vec::new());
                    put_reader(keys, v, id, key);
                }
            } else {
                let new: Vec<(u64, &M::Addr)> = reads.iter().map(|(h, a)| (*h, a)).collect();
                let old: Vec<(u64, &M::Addr)> = old_reads.iter().map(|a| (hash64(a), a)).collect();
                let (in_new, in_old) = (ReadSet::new(&new), ReadSet::new(&old));
                for &(h, a) in &new {
                    if !in_old.contains(h, a) {
                        let v =
                            readers.entry_by(h, |k| k.0 == *a, || ByHash(a.clone()), Vec::new());
                        put_reader(keys, v, id, key);
                    }
                }
                for &(h, a) in &old {
                    if !in_new.contains(h, a)
                        && let Some(v) = readers.get_mut_by(h, |k| k.0 == *a)
                    {
                        drop_reader(keys, v, id, key);
                    }
                }
            }
            reads.into_iter().map(|(_, a)| a).collect()
        };
        let Fold { keys, defs, .. } = self;
        for &rec in &recs {
            for (ix, a) in writes(rec).iter().enumerate() {
                let e = Entry {
                    step: id,
                    rec,
                    ix: u32::try_from(ix).expect("fewer than 2^32 writes"),
                };
                // (over the step's entry: its last run's, or an earlier
                // record's of this run, the step's definition being its
                // last)
                let v = defs.entry_by(hash64(a), |k| k.0 == *a, || ByHash(a.clone()), Vec::new());
                if v.last().is_none_or(|l| keys[l.step as usize] < key) {
                    v.push(e);
                } else {
                    let at = v.partition_point(|x| keys[x.step as usize] < key);
                    match v.get_mut(at) {
                        Some(x) if x.step == id => *x = e,
                        _ => v.insert(at, e),
                    }
                }
            }
        }
        // (the last run's definitions this run did not make again: an entry
        // still naming one of its records, which this run did not make
        // again; a record it did, a hit, put its writes over them)
        let mut made = recs.clone();
        made.sort_unstable();
        for &rec in old_recs.iter().filter(|r| made.binary_search(r).is_err()) {
            for a in writes(rec) {
                if let Some(v) = defs.get_mut_by(hash64(&a), |k| k.0 == a) {
                    let at = v.partition_point(|x| keys[x.step as usize] < key);
                    if v.get(at)
                        .is_some_and(|x| x.step == id && made.binary_search(&x.rec).is_err())
                    {
                        v.remove(at);
                    }
                }
            }
        }
        let s = &mut self.steps[id as usize];
        s.recs = recs;
        s.reads = addrs;
    }

    /// The definition of `a` that reaches key `key`: the last live one
    /// before it (`None`: the slot's value before the build defined it).
    #[must_use]
    pub fn reaching(&self, a: &M::Addr, key: u64) -> Option<Def> {
        let v = self.defs.get_by(hash64(a), |k| k.0 == *a)?;
        let at = v.partition_point(|x| self.key_of(x.step) < key);
        v[..at]
            .iter()
            .rev()
            .find(|e| self.live(e))
            .map(|e| self.def(e))
    }

    /// The last live definition of `a`.
    #[must_use]
    pub fn latest(&self, a: &M::Addr) -> Option<Def> {
        let v = self.defs.get_by(hash64(a), |k| k.0 == *a)?;
        v.iter().rev().find(|e| self.live(e)).map(|e| self.def(e))
    }

    /// The first live definition of `a` after key `key`.
    #[must_use]
    pub fn next_after(&self, a: &M::Addr, key: u64) -> Option<Def> {
        let v = self.defs.get_by(hash64(a), |k| k.0 == *a)?;
        let at = v.partition_point(|x| self.key_of(x.step) <= key);
        v[at..].iter().find(|e| self.live(e)).map(|e| self.def(e))
    }

    fn def(&self, e: &Entry) -> Def {
        Def {
            step: e.step,
            key: self.key_of(e.step),
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
        let from = v.partition_point(|&s| self.key_of(s) <= lo);
        v[from..]
            .iter()
            .copied()
            .take_while(|&s| hi.is_none_or(|h| self.key_of(s) <= h))
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

/// Put reader `id`, whose key is `key`, in `v` (in the order of the
/// steps' keys, `keys`) if it is not there. A cold build's steps come in
/// key order: each at the end.
fn put_reader(keys: &[u64], v: &mut Vec<StepId>, id: StepId, key: u64) {
    if v.last().is_none_or(|&l| keys[l as usize] < key) {
        v.push(id);
        return;
    }
    let at = v.partition_point(|&s| keys[s as usize] < key);
    if v.get(at) != Some(&id) {
        v.insert(at, id);
    }
}

/// Remove reader `id`, whose key is `key`, from `v`.
fn drop_reader(keys: &[u64], v: &mut Vec<StepId>, id: StepId, key: u64) {
    let at = v.partition_point(|&s| keys[s as usize] < key);
    if v.get(at) == Some(&id) {
        v.remove(at);
    }
}

/// A run's reads, each with its address's hash, to look up by address:
/// open addressing over the hashes, each place a read's index + 1 (0:
/// empty), at least twice as many places as reads.
struct ReadSet<'a, A> {
    reads: &'a [(u64, &'a A)],
    places: Vec<u32>,
    mask: usize,
}

impl<'a, A: Eq> ReadSet<'a, A> {
    fn new(reads: &'a [(u64, &'a A)]) -> Self {
        let n = (reads.len() * 2).next_power_of_two().max(16);
        let mut places = alloc::vec![0u32; n];
        let mask = n - 1;
        for (i, &(h, _)) in reads.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation, reason = "the hash's low bits")]
            let mut p = h as usize & mask;
            while places[p] != 0 {
                p = (p + 1) & mask;
            }
            places[p] = u32::try_from(i + 1).expect("fewer than 2^32 reads");
        }
        ReadSet {
            reads,
            places,
            mask,
        }
    }

    fn contains(&self, h: u64, a: &A) -> bool {
        #[allow(clippy::cast_possible_truncation, reason = "the hash's low bits")]
        let mut p = h as usize & self.mask;
        loop {
            match self.places[p] {
                0 => return false,
                i => {
                    let (rh, ra) = self.reads[i as usize - 1];
                    if rh == h && ra == a {
                        return true;
                    }
                }
            }
            p = (p + 1) & self.mask;
        }
    }
}
