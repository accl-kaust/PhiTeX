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

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::hash::hash64;
use crate::machine::Machine;
use crate::runtime::RecId;
use crate::table::{ByHash, Table};

/// A step's id: its index in [`Fold::steps`], never reused.
pub type StepId = u32;

/// A slot's id: its index in [`Fold::slot`]'s table, each slot the fold
/// has seen read once, never reused.
pub type SlotId = u32;

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
    /// The slots it read from outside it, each once, in the order read,
    /// by id ([`Fold::slot`]; [`Fold::reads_of`]).
    pub reads: Vec<SlotId>,
    _addr: core::marker::PhantomData<A>,
    /// Which run of the step made its entries (a run again bumps it).
    pub run: u32,
    /// The serial its latest run began at ([`crate::Runtime::open_step_serial`]):
    /// what the engine made in an older run of it is not the program's.
    pub serial: u64,
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
    /// Whether each step is live, a bit by id, as [`Step::live`] holds
    /// it: the searches test each entry's step here, a few kilobytes for
    /// the fold, where the steps are megabytes.
    alive: Vec<u64>,
    /// The live steps in program order.
    pub order: Vec<StepId>,
    /// Each slot's definitions, by the address's hash, in the order of
    /// the steps' keys.
    defs: Table<ByHash<M::Addr>, Vec<Entry>>,
    /// Each slot's readers, by the address's hash: the live steps whose
    /// last run read it from outside them, in the order of their keys. An
    /// entry is the step's id alone (4 bytes where a definition's is 24: a
    /// slot every step reads has a list as long as the fold), its key the
    /// step's, so keys made again leave the lists as they are; a long
    /// list in chunks ([`Readers`]).
    readers: Vec<Readers>,
    /// The slots read from outside a step, each once, by id, and each
    /// one's id: a step's read is its slot's id, 4 bytes where an
    /// address is 16 (a thesis's steps read 12.8 M times, 384 K slots).
    slots: Vec<M::Addr>,
    slot_ids: Table<ByHash<M::Addr>, SlotId>,
    /// How many times the keys were made again ([`Fold::renumber`]).
    pub renumbered: u32,
    /// The steps removed since [`Fold::release_removed`].
    removed: Vec<StepId>,
    /// The first step's definitions ([`BASE`], the job's start: the
    /// format's state, most slots' only definition), out of `defs`: each
    /// a record and a write's index packed ([`pack`]), by the slot's place
    /// in its family's array ([`Machine::dense`]), or by address for a
    /// slot with none. In `defs`, 633 K slots of a thesis's 826 K, each a
    /// list of its own and a place in the table, held 150 MB.
    base: Vec<Vec<u64>>,
    base_sparse: Table<ByHash<M::Addr>, u64>,
}

/// The step whose definitions are kept apart ([`Fold::base`]): the first.
pub const BASE: StepId = 0;

/// No definition, in [`Fold::base`]'s arrays.
const NO_DEF: u64 = u64::MAX;

fn pack(rec: RecId, ix: u32) -> u64 {
    u64::from(rec) << 32 | u64::from(ix)
}

fn unpack(x: u64) -> (RecId, u32) {
    #[allow(clippy::cast_possible_truncation, reason = "the two halves")]
    let r = ((x >> 32) as u32, x as u32);
    r
}

impl<M: Machine> Default for Fold<M> {
    fn default() -> Self {
        Fold {
            steps: Vec::new(),
            keys: Vec::new(),
            alive: Vec::new(),
            order: Vec::new(),
            defs: Table::new(),
            readers: Vec::new(),
            slots: Vec::new(),
            slot_ids: Table::new(),
            renumbered: 0,
            removed: Vec::new(),
            base: Vec::new(),
            base_sparse: Table::new(),
        }
    }
}

impl<M: Machine> Fold<M> {
    fn live(&self, e: &Entry) -> bool {
        let s = e.step as usize;
        self.alive
            .get(s / 64)
            .is_some_and(|w| w >> (s % 64) & 1 != 0)
    }

    /// Step `s`'s key.
    fn key_of(&self, s: StepId) -> u64 {
        self.keys[s as usize]
    }

    /// The slot of id `i`.
    #[must_use]
    pub fn slot(&self, i: SlotId) -> &M::Addr {
        &self.slots[i as usize]
    }

    /// The slots step `s` read from outside it, in the order read.
    pub fn reads_of(&self, s: StepId) -> impl Iterator<Item = &M::Addr> + '_ {
        self.steps
            .get(s as usize)
            .map_or(&[][..], |x| &x.reads[..])
            .iter()
            .map(|&i| &self.slots[i as usize])
    }

    /// `a`'s id, if the fold has seen it read.
    fn find_slot(&self, a: &M::Addr) -> Option<SlotId> {
        self.slot_ids.get_by(hash64(a), |k| k.0 == *a).copied()
    }

    /// `a`'s id (`h` its hash), made if it has none.
    fn slot_id(&mut self, h: u64, a: &M::Addr) -> SlotId {
        if let Some(&i) = self.slot_ids.get_by(h, |k| k.0 == *a) {
            return i;
        }
        let i = SlotId::try_from(self.slots.len()).expect("fewer than 2^32 slots");
        self.slots.push(a.clone());
        self.readers.push(Readers::default());
        self.slot_ids.insert(ByHash(a.clone()), i);
        i
    }

    /// The base step's definition of `a`, if the step is live.
    fn base_def(&self, a: &M::Addr) -> Option<Def> {
        if !self.steps.get(BASE as usize).is_some_and(|s| s.live) {
            return None;
        }
        let x = match M::dense(a) {
            Some((f, i)) => *self.base.get(f)?.get(i)?,
            None => *self.base_sparse.get_by(hash64(a), |k| k.0 == *a)?,
        };
        (x != NO_DEF).then(|| {
            let (rec, ix) = unpack(x);
            Def {
                step: BASE,
                key: self.key_of(BASE),
                rec,
                ix,
            }
        })
    }

    /// The base step's definitions, made whole again from its records'
    /// writes: its last write of each slot, but those in `skip` (sorted).
    fn close_base(
        &mut self,
        recs: &[RecId],
        writes: impl Fn(RecId) -> Vec<M::Addr>,
        skip: &[&M::Addr],
    ) {
        self.base.clear();
        self.base_sparse = Table::new();
        for &rec in recs {
            for (ix, a) in writes(rec).iter().enumerate() {
                if skip.binary_search(&a).is_err() {
                    let ix = u32::try_from(ix).expect("fewer than 2^32 writes");
                    self.set_base(a, pack(rec, ix));
                }
            }
        }
    }

    /// Make `a`'s base definition `x` ([`pack`]).
    fn set_base(&mut self, a: &M::Addr, x: u64) {
        match M::dense(a) {
            Some((f, i)) => {
                if self.base.len() <= f {
                    self.base.resize_with(f + 1, Vec::new);
                }
                let v = &mut self.base[f];
                if v.len() <= i {
                    v.resize(i + 1, NO_DEF);
                }
                v[i] = x;
            }
            None => {
                *self
                    .base_sparse
                    .entry_by(hash64(a), |k| k.0 == *a, || ByHash(a.clone()), x) = x;
            }
        }
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
        let (rs, rn, rc) = {
            let (mut n, mut cap) = (0, 0);
            for v in &self.readers {
                n += v.len();
                cap += v.capacity();
            }
            (self.readers.len(), n, cap)
        };
        // (the readers' lists by length: of a slot most steps read, a
        // list as long as the fold)
        let mut hist = [(0usize, 0usize); 6];
        for v in &self.readers {
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
        let base: usize = self.base.iter().map(Vec::capacity).sum();
        let (mut sr, mut src, mut recs, mut dead) = (0, 0, 0, 0);
        for s in &self.steps {
            sr += s.reads.len();
            src += s.reads.capacity();
            recs += s.recs.capacity();
            if !s.live {
                dead += s.reads.capacity();
            }
        }
        let (a, e, r) = (size_of::<SlotId>(), size_of::<Entry>(), size_of::<StepId>());
        alloc::format!(
            "fold: {} steps ({} B each, {} MB); their reads {sr} (capacity {src}, {} MB; the removed steps' {dead}), records {recs}, {} live; slots read {} ({} B each); readers: {rs} slots, {rn} entries (capacity {rc}, {r} B: {} MB), by a list's length (slots, entries) <=4 {:?}, <=64 {:?}, <=1K {:?}, <=8K {:?}, <=32K {:?}, more {:?}; definitions: {ds} slots, {dn} entries (capacity {dc}, {e} B: {} MB), the first step's apart: {base} places, {} by address",
            self.steps.len(),
            size_of::<Step<M::Addr>>(),
            (self.steps.capacity() * size_of::<Step<M::Addr>>()) >> 20,
            (src * a) >> 20,
            self.order.len(),
            self.slots.len(),
            size_of::<M::Addr>(),
            (rc * r) >> 20,
            hist[0],
            hist[1],
            hist[2],
            hist[3],
            hist[4],
            hist[5],
            (dc * e) >> 20,
            self.base_sparse.len(),
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
            _addr: core::marker::PhantomData,
            run: 0,
            serial: 0,
            live: true,
        });
        self.keys.push(key);
        let s = id as usize;
        if self.alive.len() <= s / 64 {
            self.alive.resize(s / 64 + 1, 0);
        }
        self.alive[s / 64] |= 1 << (s % 64);
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
        self.readers.iter_mut().for_each(|v| v.retain(live));
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
        for &i in &self.steps[id as usize].reads {
            if let Some(v) = self.readers.get_mut(i as usize) {
                v.drop(&self.keys, id, key);
            }
        }
        for a in written {
            if let Some(v) = self.defs.get_mut_by(hash64(a), |k| k.0 == *a) {
                let at = first_not_below(v, |x| self.keys[x.step as usize], key);
                if v.get(at).is_some_and(|x| x.step == id) {
                    v.remove(at);
                }
            }
        }
        self.steps[id as usize].live = false;
        self.removed.push(id);
        let s = id as usize;
        if let Some(w) = self.alive.get_mut(s / 64) {
            *w &= !(1 << (s % 64));
        }
    }

    /// Let the steps removed since the last call go: their reads and
    /// records, which the rebuild that passed over them read while it
    /// ran (its new steps' predictions), and nothing reads after it. Kept,
    /// a thesis's settling held 15 M reads of removed steps, half the
    /// fold's, and records the collector had freed.
    pub fn release_removed(&mut self) {
        for id in core::mem::take(&mut self.removed) {
            let s = &mut self.steps[id as usize];
            if !s.live {
                s.reads = Vec::new();
                s.recs = Vec::new();
            }
        }
    }

    /// Let the records of the steps removed since
    /// [`Fold::release_removed`] go (their reads stay, for the rebuild's
    /// predictions, until it ends).
    pub fn release_removed_records(&mut self) {
        for &id in &self.removed {
            let s = &mut self.steps[id as usize];
            if !s.live {
                s.recs = Vec::new();
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
    /// the fold, kept in chunks ([`Readers`]), and an insertion moves the
    /// tail of one chunk.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn close(
        &mut self,
        id: StepId,
        recs: Vec<RecId>,
        reads: Vec<(u64, M::Addr)>,
        writes: impl Fn(RecId) -> Vec<M::Addr>,
        skip: &[M::Addr],
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
        // (exactly as long: collected in place from the pairs, the ids
        // would keep their room)
        let mut ids: Vec<SlotId> = Vec::with_capacity(reads.len());
        for (h, a) in &reads {
            ids.push(self.slot_id(*h, a));
        }
        drop(reads);
        let addrs = if ids == old_reads {
            old_reads
        } else {
            let Fold { keys, readers, .. } = self;
            if old_reads.is_empty() {
                for &i in &ids {
                    readers[i as usize].put(keys, id, key);
                }
            } else {
                let mut in_new = ids.clone();
                in_new.sort_unstable();
                let mut in_old = old_reads.clone();
                in_old.sort_unstable();
                for &i in &ids {
                    if in_old.binary_search(&i).is_err() {
                        readers[i as usize].put(keys, id, key);
                    }
                }
                for &i in &old_reads {
                    if in_new.binary_search(&i).is_err() {
                        readers[i as usize].drop(keys, id, key);
                    }
                }
            }
            ids
        };
        // (sorted, for a search per write)
        let mut skip_sorted: Vec<&M::Addr> = skip.iter().collect();
        skip_sorted.sort_unstable();
        if id == BASE {
            self.close_base(&recs, writes, &skip_sorted);
            (self.steps[id as usize].recs, self.steps[id as usize].reads) = (recs, addrs);
            return;
        }
        let Fold { keys, defs, .. } = self;
        for &rec in &recs {
            for (ix, a) in writes(rec).iter().enumerate() {
                // (a slot the run left as it found it, read only softly:
                // not its definition, `Runtime::end_step_soft`; the index
                // stays the record's)
                if skip_sorted.binary_search(&a).is_ok() {
                    continue;
                }
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
                    // (room for one first: most slots have one definition)
                    v.reserve_exact(usize::from(v.capacity() == 0));
                    v.push(e);
                } else {
                    let at = first_not_below(v, |x| keys[x.step as usize], key);
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
                    let at = first_not_below(v, |x| keys[x.step as usize], key);
                    if v.get(at)
                        .is_some_and(|x| x.step == id && made.binary_search(&x.rec).is_err())
                    {
                        v.remove(at);
                    }
                }
            }
        }
        // (and the step's entries of the slots skipped, a record made
        // again having left its last run's in place)
        for a in skip {
            if let Some(v) = defs.get_mut_by(hash64(a), |k| k.0 == *a) {
                let at = first_not_below(v, |x| keys[x.step as usize], key);
                if v.get(at).is_some_and(|x| x.step == id) {
                    v.remove(at);
                }
            }
        }
        let s = &mut self.steps[id as usize];
        s.recs = recs;
        s.reads = addrs;
    }

    /// Whether step `id` has a definition of `a` in the index (a slot it
    /// left as it found it, read only softly, has none:
    /// `Runtime::end_step_soft`).
    #[must_use]
    pub fn defines(&self, a: &M::Addr, id: StepId) -> bool {
        if id == BASE {
            return self.base_def(a).is_some();
        }
        let Some(key) = self.steps.get(id as usize).map(|s| s.key) else {
            return false;
        };
        let Some(v) = self.defs.get_by(hash64(a), |k| k.0 == *a) else {
            return false;
        };
        let at = first_not_below(v, |x| self.key_of(x.step), key);
        v.get(at).is_some_and(|x| x.step == id)
    }

    /// Step `id`'s definition of `a` in the index: its record and the
    /// write's index in it.
    #[must_use]
    pub fn entry_of(&self, a: &M::Addr, id: StepId) -> Option<(RecId, u32)> {
        if id == BASE {
            return self.base_def(a).map(|d| (d.rec, d.ix));
        }
        let key = self.steps.get(id as usize)?.key;
        let v = self.defs.get_by(hash64(a), |k| k.0 == *a)?;
        let at = first_not_below(v, |x| self.key_of(x.step), key);
        v.get(at).filter(|x| x.step == id).map(|x| (x.rec, x.ix))
    }

    /// The definition of `a` that reaches key `key`: the last live one
    /// before it (`None`: the slot's value before the build defined it).
    #[must_use]
    pub fn reaching(&self, a: &M::Addr, key: u64) -> Option<Def> {
        // (else the base step's, the first)
        self.defs
            .get_by(hash64(a), |k| k.0 == *a)
            .and_then(|v| {
                let at = first_not_below(v, |x| self.key_of(x.step), key);
                v[..at].iter().rev().find(|e| self.live(e))
            })
            .map(|e| self.def(e))
            .or_else(|| {
                // (the base step's, the first, if it is before `key`)
                (self.keys.first().is_some_and(|&k| k < key))
                    .then(|| self.base_def(a))
                    .flatten()
            })
    }

    /// The last live definition of `a`.
    #[must_use]
    pub fn latest(&self, a: &M::Addr) -> Option<Def> {
        self.defs
            .get_by(hash64(a), |k| k.0 == *a)
            .and_then(|v| v.iter().rev().find(|e| self.live(e)))
            .map(|e| self.def(e))
            .or_else(|| self.base_def(a))
    }

    /// The definition of `a` that reaches key `key` ([`Fold::reaching`]),
    /// if a live one is at or after `key` (`None`: none is, and the latest
    /// reaches it): the test and the search with one lookup of the slot.
    #[must_use]
    pub fn reaching_if_later(&self, a: &M::Addr, key: u64) -> Option<Option<Def>> {
        let v = self.defs.get_by(hash64(a), |k| k.0 == *a);
        let last = v.and_then(|v| v.iter().rev().find(|e| self.live(e)));
        let Some(last) = last else {
            // (the base step's alone, the first)
            let b = self.base_def(a)?;
            return (b.key >= key).then_some(None);
        };
        if self.key_of(last.step) < key {
            return None;
        }
        Some(self.reaching(a, key))
    }

    /// The first live definition of `a` after key `key`.
    #[must_use]
    pub fn next_after(&self, a: &M::Addr, key: u64) -> Option<Def> {
        if self.keys.first().is_some_and(|&k| k > key)
            && let Some(b) = self.base_def(a)
        {
            return Some(b);
        }
        let v = self.defs.get_by(hash64(a), |k| k.0 == *a)?;
        let at = first_above(v, |x| self.key_of(x.step), key);
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
        let Some(v) = self.find_slot(a).map(|i| &self.readers[i as usize]) else {
            return Vec::new();
        };
        v.after(&self.keys, lo)
            .take_while(|&s| hi.is_none_or(|h| self.key_of(s) <= h))
            .collect()
    }

    /// Step `id`'s position in [`Fold::order`].
    #[must_use]
    pub fn position(&self, id: StepId) -> Option<usize> {
        let key = self.steps.get(id as usize)?.key;
        let at = first_not_below(&self.order, |&s| self.steps[s as usize].key, key);
        (self.order.get(at) == Some(&id)).then_some(at)
    }

    /// The records of the live steps, for the collector's roots.
    pub fn roots(&self) -> impl Iterator<Item = RecId> + '_ {
        self.order
            .iter()
            .flat_map(|&s| self.steps[s as usize].recs.iter().copied())
    }
}

/// The first index of `v`, in increasing order of `key_of`, whose key is
/// not below `key`: `v.partition_point(|x| key_of(x) < key)`, found by
/// guessing where keys spread evenly would put it (the fold's are, and
/// renumbering keeps them so), galloping from the guess, then halving. A
/// list as long as the fold (a slot every step reads or defines) takes a
/// few probes, each a load of a step's key, not its length's logarithm.
fn first_not_below<T>(v: &[T], key_of: impl Fn(&T) -> u64, key: u64) -> usize {
    let n = v.len();
    if n < 32 {
        return v.partition_point(|x| key_of(x) < key);
    }
    let (lo, hi) = (key_of(&v[0]), key_of(&v[n - 1]));
    if key <= lo {
        return 0;
    }
    if key > hi {
        return n;
    }
    // (lo < key <= hi: the guess is in [0, n - 1]; a float's division,
    // where a 128-bit one is a call)
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a guess: any index in range is right, and the cast saturates"
    )]
    let g = (((key - lo) as f64 / (hi - lo) as f64) * (n - 1) as f64) as usize;
    let g = g.min(n - 1);
    let below = |i: usize| key_of(&v[i]) < key;
    // (below at `a`, not at `b`: v[0] is below, v[n - 1] is not)
    let (mut a, mut b);
    let mut step = 1;
    if below(g) {
        a = g;
        loop {
            b = (a + step).min(n - 1);
            if !below(b) {
                break;
            }
            a = b;
            step *= 2;
        }
    } else {
        b = g;
        loop {
            a = b.saturating_sub(step);
            if below(a) {
                break;
            }
            b = a;
            step *= 2;
        }
    }
    a + 1 + v[a + 1..b].partition_point(|x| key_of(x) < key)
}

/// The first index of `v` whose key is above `key`
/// (`v.partition_point(|x| key_of(x) <= key)`, [`first_not_below`]).
fn first_above<T>(v: &[T], key_of: impl Fn(&T) -> u64, key: u64) -> usize {
    key.checked_add(1)
        .map_or(v.len(), |k| first_not_below(v, key_of, k))
}

/// A flat list of readers longer than this is kept in chunks
/// ([`Readers`]).
const FLAT_MAX: usize = 512;

/// A chunk's length when a list is cut into chunks, and when a chunk
/// at the end is full: one put there starts another.
const CHUNK: usize = 512;

/// A slot's readers, in the order of the steps' keys (each a [`StepId`],
/// its key the step's). A short list is one vector. A long one, of a slot
/// every step reads (a catcode, `\baselineskip`), is cut into chunks of
/// at most `2 * CHUNK`. A rebuild's new step goes into the middle of the
/// list; in one vector as long as the fold, that moved the tail at each
/// put (a thesis's settle: 64 K steps, a few hundred such slots per new
/// step), and now it moves at most a chunk's.
pub(crate) enum Readers {
    Flat(Vec<StepId>),
    /// Each chunk not empty, in order; boxed, so a list is as big as a
    /// vector (most slots have a few readers).
    #[allow(
        clippy::box_collection,
        reason = "unboxed, every slot's list is 32 bytes, not 24"
    )]
    Chunks(Box<Vec<Vec<StepId>>>),
}

impl Default for Readers {
    fn default() -> Self {
        Readers::Flat(Vec::new())
    }
}

impl Readers {
    fn len(&self) -> usize {
        match self {
            Readers::Flat(v) => v.len(),
            Readers::Chunks(c) => c.iter().map(Vec::len).sum(),
        }
    }

    fn capacity(&self) -> usize {
        match self {
            Readers::Flat(v) => v.capacity(),
            Readers::Chunks(c) => c.iter().map(Vec::capacity).sum(),
        }
    }

    /// The chunk where key `key` is or goes: the first whose last key is
    /// not below it, else the last.
    fn chunk(chunks: &[Vec<StepId>], keys: &[u64], key: u64) -> usize {
        chunks
            .partition_point(|c| c.last().is_some_and(|&l| keys[l as usize] < key))
            .min(chunks.len().saturating_sub(1))
    }

    /// Put reader `id`, whose key is `key`, in the list if it is not
    /// there. A cold build's steps come in key order: each at the end.
    fn put(&mut self, keys: &[u64], id: StepId, key: u64) {
        match self {
            Readers::Flat(v) => {
                put_reader(keys, v, id, key);
                if v.len() > FLAT_MAX {
                    let chunks = v.chunks(CHUNK).map(<[StepId]>::to_vec).collect();
                    *self = Readers::Chunks(Box::new(chunks));
                }
            }
            Readers::Chunks(c) => {
                let at = Self::chunk(c, keys, key);
                let last = at + 1 == c.len();
                let Some(v) = c.get_mut(at) else {
                    c.push(alloc::vec![id]);
                    return;
                };
                if last && v.len() >= CHUNK && keys[v[v.len() - 1] as usize] < key {
                    // (past the end, the last chunk full: a new one)
                    let mut n = Vec::with_capacity(CHUNK);
                    n.push(id);
                    c.push(n);
                    return;
                }
                put_reader(keys, v, id, key);
                if v.len() > 2 * CHUNK {
                    let tail = v.split_off(CHUNK);
                    v.shrink_to_fit();
                    c.insert(at + 1, tail);
                }
            }
        }
    }

    /// Remove reader `id`, whose key is `key`.
    fn drop(&mut self, keys: &[u64], id: StepId, key: u64) {
        match self {
            Readers::Flat(v) => drop_reader(keys, v, id, key),
            Readers::Chunks(c) => {
                let at = Self::chunk(c, keys, key);
                let Some(v) = c.get_mut(at) else { return };
                drop_reader(keys, v, id, key);
                if v.is_empty() {
                    c.remove(at);
                }
            }
        }
    }

    /// Keep the readers `keep` says to.
    fn retain(&mut self, keep: impl Fn(StepId) -> bool) {
        match self {
            Readers::Flat(v) => v.retain(|&s| keep(s)),
            Readers::Chunks(c) => {
                for v in c.iter_mut() {
                    v.retain(|&s| keep(s));
                }
                c.retain(|v| !v.is_empty());
            }
        }
    }

    /// The readers with keys above `lo`, in order.
    fn after<'a>(&'a self, keys: &'a [u64], lo: u64) -> impl Iterator<Item = StepId> + 'a {
        let key_of = |&s: &StepId| keys[s as usize];
        let (first, rest): (&[StepId], &[Vec<StepId>]) = match self {
            Readers::Flat(v) => (&v[first_above(v, key_of, lo)..], &[]),
            Readers::Chunks(c) => match lo.checked_add(1) {
                Some(k) if !c.is_empty() => {
                    let at = Self::chunk(c, keys, k);
                    let v = &c[at];
                    (&v[first_above(v, key_of, lo)..], &c[at + 1..])
                }
                _ => (&[], &[]),
            },
        };
        first.iter().chain(rest.iter().flatten()).copied()
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
    let at = first_not_below(v, |&s| keys[s as usize], key);
    if v.get(at) != Some(&id) {
        v.insert(at, id);
    }
}

/// Remove reader `id`, whose key is `key`, from `v`.
fn drop_reader(keys: &[u64], v: &mut Vec<StepId>, id: StepId, key: u64) {
    let at = first_not_below(v, |&s| keys[s as usize], key);
    if v.get(at) == Some(&id) {
        v.remove(at);
    }
}

#[cfg(test)]
mod tests {
    use super::{CHUNK, Readers, StepId};
    use alloc::vec::Vec;

    /// [`Readers`] against one sorted vector: puts at the end (a cold
    /// build), in the middle (a rebuild's new steps), drops, retains and
    /// ranges, past the chunks' splits.
    #[test]
    fn readers_as_one_list() {
        // (step `s`'s key: the steps put first get every 1000th key, the
        // later ones keys between them)
        let mut keys: Vec<u64> = Vec::new();
        let mut r = Readers::default();
        let mut want: Vec<StepId> = Vec::new();
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let check = |r: &Readers, want: &[StepId], keys: &[u64], lo: u64| {
            let got: Vec<StepId> = r.after(keys, lo).collect();
            let exp: Vec<StepId> = want
                .iter()
                .copied()
                .filter(|&s| keys[s as usize] > lo)
                .collect();
            assert_eq!(got, exp);
            assert_eq!(r.len(), want.len());
        };
        for i in 0..(5 * CHUNK as u64) {
            keys.push((i + 1) * 1000);
            let id = StepId::try_from(keys.len() - 1).unwrap();
            r.put(&keys, id, keys[id as usize]);
            want.push(id);
        }
        check(&r, &want, &keys, 0);
        for round in 0..20_000 {
            let k = rand() % (5 * CHUNK as u64 * 1000 + 2000);
            match rand() % 3 {
                // (keys are unique, as the fold's)
                0 if !keys.contains(&k) => {
                    keys.push(k);
                    let id = StepId::try_from(keys.len() - 1).unwrap();
                    r.put(&keys, id, k);
                    let at = want.partition_point(|&s| keys[s as usize] < k);
                    want.insert(at, id);
                }
                1 if !want.is_empty() => {
                    let id = want[usize::try_from(rand()).unwrap() % want.len()];
                    r.drop(&keys, id, keys[id as usize]);
                    want.retain(|&s| s != id);
                }
                _ => {
                    // (a put again of one there changes nothing)
                    if let Some(&id) = want.first() {
                        r.put(&keys, id, keys[id as usize]);
                    }
                }
            }
            if round % 500 == 0 {
                check(&r, &want, &keys, k);
            }
        }
        r.retain(|s| s % 3 != 0);
        want.retain(|&s| s % 3 != 0);
        check(&r, &want, &keys, 0);
        check(&r, &want, &keys, u64::MAX);
    }
}
