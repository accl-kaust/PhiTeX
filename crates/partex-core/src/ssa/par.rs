//! Steps on workers (DESIGN 3.10, "Threads", and 4.9): a step runs on a
//! worker's view of the engine from the entry state the run before it
//! predicts, and the build takes its run in program order, at the step's
//! turn, if every slot it read from outside it holds there the version it
//! read (its commit); one that does not runs again in place, as it would
//! with one worker. So the outputs are a sequential build's whatever the
//! number of workers and their timing.
//!
//! A worker's view is a copy-on-write fork of the engine
//! ([`Tex::fork_with`]: eqtb, its objects, the hash and the save stack
//! shared by chunks, a write copying the one it lands in) and a recorder
//! of its own ([`SsaTracker::worker`]) whose tables of names, files and
//! codes read through to the build's ([`Base`]) and whose versions of
//! the tables' slots are made from their contents, as the build's are.
//! What the worker's run cannot hand over as values (a font loaded, a
//! command run, a file opened anew, an error) taints it ([`Taint`]): the
//! step then runs at its turn as before.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, Ordering};

use partex_ssa::Version;
#[cfg(feature = "std")]
use partex_ssa::fold::StepId;

#[cfg(feature = "std")]
use super::Step;
use super::{Fam, LineCodes, RecState, Recorder, SVal, Slot, SsaTracker, TexSsa};
#[cfg(feature = "std")]
use crate::tex::Tex;

/// A font slot as a worker sees it, from the build's fold when the round
/// began: its maker's key and its count among the fonts made, if a live
/// step's latest run made it; `None`: a format's font.
pub(crate) type FontAt = Option<(u64, u64, bool)>;

/// What a worker's tracker reads of the build, and what its run did that
/// a commit cannot take (DESIGN 3.10, "Per worker").
pub(crate) struct Worker {
    /// The key of the step it runs, in the build's fold.
    pub(super) key: Cell<u64>,
    /// The step's salt: what its run names what it makes by (its seals,
    /// its PDF objects).
    pub(super) salt: Cell<u32>,
    /// The fonts' makers ([`FontAt`]).
    pub(super) fonts: Arc<Vec<FontAt>>,
    /// The load ids of the names the build stores.
    pub(super) stored: Arc<BTreeSet<u32>>,
    /// The names the build entered, by hash slot, and each one's maker
    /// key and name id (`SsaTracker::made_at`, `made`).
    pub(super) made_at: Arc<Vec<u32>>,
    pub(super) made: Arc<Vec<(u64, u32)>>,
    /// The names its run entered: the hash slot and the name's id.
    pub(super) made_names: RefCell<Vec<(i32, u32)>>,
    /// Why its run cannot be taken, if it cannot.
    pub(super) tainted: Cell<Option<&'static str>>,
    /// The round's stop: set when no commit will want its run.
    pub(super) stop: Arc<AtomicBool>,
    /// Whether its run asked where the fonts were made ([`Worker::fonts`]),
    /// and the hash slots whose maker it asked ([`Worker::made_at`]): the
    /// commit compares the build's answers then with these.
    pub(super) asked_fonts: Cell<bool>,
    pub(super) asked_made: RefCell<Vec<i32>>,
}

impl Worker {
    /// A worker's tracker state for the step at `key`, named `salt`.
    pub(super) fn new(key: u64, salt: u32, snap: &Snapshot) -> Worker {
        Worker {
            key: Cell::new(key),
            salt: Cell::new(salt),
            fonts: snap.fonts.clone(),
            stored: snap.stored.clone(),
            made_at: snap.made_at.clone(),
            made: snap.made.clone(),
            made_names: RefCell::new(Vec::new()),
            tainted: Cell::new(None),
            stop: snap.stop.clone(),
            asked_fonts: Cell::new(false),
            asked_made: RefCell::new(Vec::new()),
        }
    }

    /// The run cannot be taken (`why`): the step runs at its turn.
    pub(super) fn taint(&self, why: &'static str) {
        if self.tainted.get().is_none() {
            self.tainted.set(Some(why));
        }
    }

    /// Whether the round no longer wants the run.
    pub(super) fn cancelled(&self) -> bool {
        self.tainted.get().is_some() || self.stop.load(Ordering::Relaxed)
    }

    /// [`crate::track::Tracker::font_visible`] at the step's key.
    pub(super) fn font_visible(&self, f: i32) -> bool {
        self.asked_fonts.set(true);
        match usize::try_from(f).ok().and_then(|i| self.fonts.get(i)) {
            Some(Some((key, _, live))) => *live && *key < self.key.get(),
            _ => true,
        }
    }

    /// [`crate::track::Tracker::font_newest`] at the step's key: whether
    /// font `f` is the newest of the fonts made by now.
    pub(super) fn font_newest(&self, f: i32) -> Option<bool> {
        self.asked_fonts.set(true);
        let k = self.key.get();
        let newest = self
            .fonts
            .iter()
            .enumerate()
            .filter_map(|(g, at)| match at {
                Some((key, n, true)) if *key < k => Some(((*key, *n), g)),
                _ => None,
            })
            .max()?
            .1;
        Some(usize::try_from(f).is_ok_and(|i| i == newest))
    }
}

/// The build's workers: how many, and the trackers kept between rounds
/// (a tracker's stamps are arrays of the tables' size, made once).
#[derive(Default)]
pub struct Par {
    /// Workers (1: none, the steps run in turn).
    pub workers: core::cell::Cell<usize>,
    /// The fewest commands a step's last run ran for it to go to a
    /// worker, and the fewest a round's steps ran together (`None`: the
    /// defaults; 0 and 0 send every dirty step, a test's).
    pub thresholds: core::cell::Cell<Option<(u64, u64)>>,
    /// A cold build's chunks on the workers too (`cold.rs`; on unless
    /// turned off).
    pub cold: core::cell::Cell<bool>,
    /// The trackers of the workers, between rounds.
    pub(super) shells: RefCell<Vec<super::SsaTracker>>,
    /// What the rounds did: steps run on workers, taken at their commit,
    /// run again at their turn (and why), and the commands each.
    pub stats: RefCell<ParStats>,
}

/// What a build's workers did (the reports' numbers).
#[derive(Clone, Debug, Default)]
pub struct ParStats {
    pub rounds: u64,
    pub runs: u64,
    pub taken: u64,
    pub rerun: u64,
    pub tainted: u64,
    pub commands_taken: u64,
    pub commands_wasted: u64,
    /// The reasons the runs were not taken, by count.
    pub why: BTreeMap<&'static str, u64>,
    /// A cold build in chunks (`B`): its passes, and when (ns from the
    /// build's start) a worker first shipped a page.
    pub passes: u64,
    pub first_page_ns: Option<u64>,
    /// Time making the workers' views, and waiting for the rounds.
    pub fork_ns: u64,
    pub round_ns: u64,
}

impl ParStats {
    /// A run not taken, for `why`.
    pub(super) fn not_taken(&mut self, why: &'static str, commands: u64) {
        self.rerun += 1;
        self.commands_wasted += commands;
        *self.why.entry(why).or_default() += 1;
    }
}

/// The build recorder's interned tables, shared with the workers while
/// they run ([`RecState::base`]): moved out of the recorder for a round
/// of workers and back after it, so the build pays no copy.
#[derive(Default)]
pub(crate) struct Base {
    pub(super) codes: Vec<LineCodes>,
    pub(super) codes_ix: BTreeMap<u128, u32>,
    pub(super) names: Vec<Vec<u8>>,
    pub(super) names_ix: BTreeMap<Vec<u8>, u32>,
    pub(super) loads: Vec<(Vec<u8>, Version, crate::host::FileKind)>,
    pub(super) loads_ix: BTreeMap<Vec<u8>, u32>,
    pub(super) searches: Vec<Vec<u8>>,
    pub(super) searches_ix: BTreeMap<Vec<u8>, u32>,
    pub(super) words: Vec<Vec<u8>>,
    pub(super) words_ix: BTreeMap<Vec<u8>, u32>,
    pub(super) written: BTreeSet<Vec<u8>>,
}

impl Base {
    /// The build recorder's tables, moved out for a round.
    pub(super) fn lend(st: &mut RecState) -> Base {
        use core::mem::take;
        Base {
            codes: take(&mut st.codes),
            codes_ix: take(&mut st.codes_ix),
            names: take(&mut st.names),
            names_ix: take(&mut st.names_ix),
            loads: take(&mut st.loads),
            loads_ix: take(&mut st.loads_ix),
            searches: take(&mut st.searches),
            searches_ix: take(&mut st.searches_ix),
            words: take(&mut st.words),
            words_ix: take(&mut st.words_ix),
            written: take(&mut st.written),
        }
    }

    /// The tables back in the build's recorder after the round (copied if
    /// a worker still holds them, which none should).
    pub(super) fn restore(base: Arc<Base>, st: &mut RecState) {
        let b = Arc::try_unwrap(base).unwrap_or_else(|b| Base {
            codes: b.codes.clone(),
            codes_ix: b.codes_ix.clone(),
            names: b.names.clone(),
            names_ix: b.names_ix.clone(),
            loads: b.loads.clone(),
            loads_ix: b.loads_ix.clone(),
            searches: b.searches.clone(),
            searches_ix: b.searches_ix.clone(),
            words: b.words.clone(),
            words_ix: b.words_ix.clone(),
            written: b.written.clone(),
        });
        st.codes = b.codes;
        st.codes_ix = b.codes_ix;
        st.names = b.names;
        st.names_ix = b.names_ix;
        st.loads = b.loads;
        st.loads_ix = b.loads_ix;
        st.searches = b.searches;
        st.searches_ix = b.searches_ix;
        st.words = b.words;
        st.words_ix = b.words_ix;
        st.written = b.written;
    }

    /// The tables' lengths: a worker's own entries are numbered from them.
    pub(super) fn lens(&self) -> BaseLens {
        BaseLens {
            codes: self.codes.len(),
            names: self.names.len(),
            loads: self.loads.len(),
            searches: self.searches.len(),
            words: self.words.len(),
        }
    }
}

/// [`Base::lens`].
#[derive(Clone, Copy, Default)]
pub(super) struct BaseLens {
    pub(super) codes: usize,
    pub(super) names: usize,
    pub(super) loads: usize,
    pub(super) searches: usize,
    pub(super) words: usize,
}

/// A worker's own interned ids, each the build's id of the same entry
/// (made in the build's tables if it has none): what a commit renames in
/// the run it takes.
#[derive(Default)]
pub(super) struct Remap {
    codes: BTreeMap<u32, u32>,
    names: BTreeMap<u32, u32>,
    loads: BTreeMap<u32, u32>,
    searches: BTreeMap<u32, u32>,
    words: BTreeMap<u32, u32>,
    /// The strings the run made, `from..to`, shifted `by` ([`Remap::shift_strings`]).
    strs: Option<(usize, usize, i64)>,
    /// How many more names the build's extra region holds than the run's
    /// did when it began ([`Remap::shift_names`]).
    high: i64,
}

/// Relocatable strings (DESIGN "Parallel builds", relocation): a run that
/// began with the pool's end at `from` and made strings `from..to` is
/// taken where the build's pool ends elsewhere, its strings numbered from
/// there. Where a run has a string's number, as a value it copies and
/// compares, the number moves with the string: the pool's slots, its end
/// (`str_ptr`, read where the run made its first string, written where it
/// left it), a name's text (`text(p)`, whose version is its bytes'), a
/// search's answer, and the input's file names. Anything that shows a
/// string's number itself would be a read of it; TeX shows strings by
/// their bytes.
impl Remap {
    /// The strings `from..to` the run made, numbered from `at` here.
    pub(super) fn shift_strings(&mut self, from: usize, to: usize, at: usize) {
        let by = i64::try_from(at).unwrap_or(0) - i64::try_from(from).unwrap_or(0);
        self.strs = (by != 0).then_some((from, to.max(from), by));
    }

    /// With names placed by name (`SsaTracker::names_by_name`), the count
    /// of the names in the hash's extra region (`hash_high`) is a count
    /// only, read by nothing but its overflow: a run's moves by the names
    /// the build holds there beyond those the run began with, `by`.
    pub(super) fn shift_names(&mut self, by: i64) {
        self.high = by;
    }

    /// String number `s`, as the build numbers it.
    pub(super) fn string(&self, s: i64) -> i64 {
        match self.strs {
            Some((from, to, by)) if usize::try_from(s).is_ok_and(|s| s >= from && s < to) => s + by,
            _ => s,
        }
    }

    /// The pool's end `s` (`str_ptr`), as the build's: one of the run's,
    /// `from..=to`, moves.
    fn pool_end(&self, s: i64) -> i64 {
        match self.strs {
            Some((from, to, by)) if usize::try_from(s).is_ok_and(|s| s >= from && s <= to) => s + by,
            _ => s,
        }
    }

    /// The version `v` a read of `a` found, as the build's strings give it.
    pub(super) fn read_version(&self, a: &Slot, v: Version) -> Version {
        let Some((from, to, _)) = self.strs else {
            return v;
        };
        match a.0 {
            Fam::Alloc if a.1 == i64::from(crate::track::scalar::STR_TOP) => {
                let n = i64::try_from(v.0 >> 1).unwrap_or(0);
                let m = usize::try_from(self.pool_end(n)).unwrap_or(0);
                Version(crate::track::scalar_version(m) | (v.0 & 1))
            }
            Fam::Search => (from..to)
                .find(|&s| Version::of(&i32::try_from(s).unwrap_or(0)) == v)
                .map_or(v, |s| {
                    let n = self.string(i64::try_from(s).unwrap_or(0));
                    Version::of(&i32::try_from(n).unwrap_or(0))
                }),
            _ => v,
        }
    }

    /// The value `v` a run's write of `a` left, as the build's strings
    /// give it: whether it moved.
    fn write_value(&self, a: &Slot, v: &mut SVal) -> bool {
        let Some(&super::SValue::Int(x)) = v.1.as_deref() else {
            return false;
        };
        let x = i64::from(x);
        if a.0 == Fam::Alloc && a.1 == i64::from(crate::track::scalar::HASH_HIGH) {
            if self.high == 0 {
                return false;
            }
            let n = i32::try_from(x + self.high).unwrap_or(0);
            *v = SVal(
                Version(crate::track::scalar_version_i32(n) | (v.0.0 & 1)),
                Some(Arc::new(super::SValue::Int(n))),
            );
            return true;
        }
        if self.strs.is_none() {
            return false;
        }
        let top = a.0 == Fam::Alloc && a.1 == i64::from(crate::track::scalar::STR_TOP);
        let n = if top { self.pool_end(x) } else { self.string(x) };
        if n == x {
            return false;
        }
        let n32 = i32::try_from(n).unwrap_or(0);
        match a.0 {
            Fam::Alloc if top => {
                let m = usize::try_from(n).unwrap_or(0);
                *v = SVal(
                    Version(crate::track::scalar_version(m) | (v.0.0 & 1)),
                    Some(Arc::new(super::SValue::Int(n32))),
                );
                true
            }
            // (a name's text: its version is its bytes', the same)
            Fam::Hash => {
                *v = SVal(v.0, Some(Arc::new(super::SValue::Int(n32))));
                true
            }
            _ => false,
        }
    }
}

impl Remap {
    /// The worker's tables `w` (numbered from `lens`) in the build's `st`.
    pub(super) fn new(w: &RecState, lens: BaseLens, st: &mut RecState) -> Remap {
        let id = |from: usize, i: usize| u32::try_from(from + i).unwrap_or(u32::MAX);
        let mut m = Remap::default();
        // (a file new to the build: its load made, at the version and as
        // the kind the worker found)
        for (i, (name, v, kind)) in w.loads.iter().enumerate() {
            let b = st.load_id(name, *v, *kind).0;
            m.loads.insert(id(lens.loads, i), b);
        }
        for (i, c) in w.codes.iter().enumerate() {
            m.codes.insert(id(lens.codes, i), st.codes_id(c));
        }
        for (i, n) in w.names.iter().enumerate() {
            m.names.insert(id(lens.names, i), st.name_id(n));
        }
        for (i, n) in w.searches.iter().enumerate() {
            m.searches.insert(id(lens.searches, i), st.search_id(n));
        }
        for (i, n) in w.words.iter().enumerate() {
            m.words.insert(id(lens.words, i), st.word(n));
        }
        m.codes.retain(|a, b| a != b);
        m.names.retain(|a, b| a != b);
        m.loads.retain(|a, b| a != b);
        m.searches.retain(|a, b| a != b);
        m.words.retain(|a, b| a != b);
        m
    }

    /// Whether it renames nothing.
    pub(super) fn is_empty(&self) -> bool {
        self.codes.is_empty()
            && self.names.is_empty()
            && self.loads.is_empty()
            && self.searches.is_empty()
            && self.words.is_empty()
            && self.strs.is_none()
            && self.high == 0
    }

    /// The renaming's version: a record renamed is the same record only
    /// under the same renaming.
    fn ver(&self) -> u128 {
        Version::of(&(
            &self.codes,
            &self.names,
            &self.loads,
            &self.searches,
            &self.words,
            self.strs,
            self.high,
        ))
        .0
    }

    /// A load id, renamed.
    pub(super) fn load(&self, i: u32) -> u32 {
        self.loads.get(&i).copied().unwrap_or(i)
    }

    /// A name id, renamed.
    pub(super) fn name(&self, i: u32) -> u32 {
        self.names.get(&i).copied().unwrap_or(i)
    }

    /// Slot `a`, renamed.
    pub(super) fn slot(&self, a: Slot) -> Slot {
        let by = |m: &BTreeMap<u32, u32>| {
            u32::try_from(a.1)
                .ok()
                .and_then(|i| m.get(&i))
                .map_or(a, |&b| Slot(a.0, i64::from(b)))
        };
        match a.0 {
            Fam::Name => by(&self.names),
            Fam::Load => by(&self.loads),
            Fam::Search => by(&self.searches),
            Fam::HyphWord => by(&self.words),
            Fam::Pool => Slot(a.0, self.string(a.1)),
            Fam::Source => {
                let c = u32::try_from((a.1 >> 32) & 0xff_ffff).unwrap_or(0);
                match self.codes.get(&c) {
                    Some(&b) => Slot(
                        a.0,
                        (a.1 & !(0xff_ffff_i64 << 32)) | (i64::from(b & 0xff_ffff) << 32),
                    ),
                    None => a,
                }
            }
            _ => a,
        }
    }

    /// The run `x`, its slots renamed; a record any of whose slots was
    /// renamed gets a content of its own (its reads' hash, which named
    /// the worker's ids, is not the build's).
    pub(super) fn export(&self, x: &mut partex_ssa::export::StepExport<TexSsa>) {
        if self.is_empty() {
            return;
        }
        let loc = |l: &mut partex_ssa::Loc<Slot>| -> bool {
            let a = match l {
                partex_ssa::Loc::State(a)
                | partex_ssa::Loc::Field(a, _)
                | partex_ssa::Loc::Phi(a) => a,
            };
            let b = self.slot(*a);
            let moved = b != *a;
            *a = b;
            moved
        };
        // (and a record above one renamed: its content holds its children's)
        let mut renamed: Vec<bool> = Vec::with_capacity(x.recs.len());
        for r in &mut x.recs {
            let mut moved = r.items.iter().any(|it| {
                matches!(it, partex_ssa::runtime::Item::Call(c) if renamed.get(*c as usize).copied().unwrap_or(false))
            });
            for (l, v) in &mut r.reads {
                moved |= loc(l);
                if let partex_ssa::Loc::State(a) = l {
                    let w = self.read_version(a, *v);
                    moved |= w != *v;
                    *v = w;
                }
            }
            for (a, v) in &mut r.writes {
                let b = self.slot(*a);
                moved |= b != *a;
                *a = b;
                if let Some(v) = v {
                    moved |= self.write_value(a, v);
                }
            }
            for it in &mut r.items {
                match it {
                    partex_ssa::runtime::Item::Open(a) | partex_ssa::runtime::Item::Wrote(a) => {
                        let b = self.slot(*a);
                        moved |= b != *a;
                        *a = b;
                    }
                    partex_ssa::runtime::Item::Store(a, _) => {
                        let b = self.slot(*a);
                        moved |= b != *a;
                        *a = b;
                    }
                    _ => {}
                }
            }
            if moved {
                r.content = Version::of(&(0x7265_6d61_70u64, r.content.0, self.ver()));
            }
            renamed.push(moved);
        }
        for (i, (h, a)) in x.reads.iter_mut().enumerate() {
            let b = self.slot(*a);
            if b != *a {
                *a = b;
                *h = partex_ssa::hash::hash64(a);
            }
            if let Some(v) = x.vers.get_mut(i) {
                *v = self.read_version(a, *v);
            }
        }
        for a in &mut x.skip {
            *a = self.slot(*a);
        }
    }
}

/// An interned table read through a base: ids below the base's length
/// are the base's, the rest this table's own, from the base's length on.
fn intern_in(
    base: Option<(&BTreeMap<Vec<u8>, u32>, usize)>,
    ix: &mut BTreeMap<Vec<u8>, u32>,
    items: &mut Vec<Vec<u8>>,
    name: &[u8],
) -> (u32, bool) {
    if let Some((b, _)) = base
        && let Some(&i) = b.get(name)
    {
        return (i, false);
    }
    if let Some(&i) = ix.get(name) {
        return (i, false);
    }
    let from = base.map_or(0, |(_, n)| n);
    let i = u32::try_from(from + items.len()).unwrap_or(u32::MAX);
    ix.insert(name.to_vec(), i);
    items.push(name.to_vec());
    (i, true)
}

/// Entry `i` of an interned table read through a base.
fn at_in<'a, T>(base: Option<&'a [T]>, items: &'a [T], i: usize) -> Option<&'a T> {
    match base {
        Some(b) if i < b.len() => b.get(i),
        Some(b) => items.get(i - b.len()),
        None => items.get(i),
    }
}

impl RecState {
    /// Control sequence name `name`'s id ([`super::Fam::Name`]).
    pub(super) fn name_id(&mut self, name: &[u8]) -> u32 {
        let b = self.base.as_deref();
        intern_in(
            b.map(|b| (&b.names_ix, b.names.len())),
            &mut self.names_ix,
            &mut self.names,
            name,
        )
        .0
    }

    /// `search_string`'s string `name`'s id ([`super::Fam::Search`]).
    pub(super) fn search_id(&mut self, name: &[u8]) -> u32 {
        let b = self.base.as_deref();
        intern_in(
            b.map(|b| (&b.searches_ix, b.searches.len())),
            &mut self.searches_ix,
            &mut self.searches,
            name,
        )
        .0
    }

    /// The exception's word `key` as an address ([`super::Fam::HyphWord`]).
    pub(super) fn word(&mut self, key: &[u8]) -> u32 {
        let b = self.base.as_deref();
        intern_in(
            b.map(|b| (&b.words_ix, b.words.len())),
            &mut self.words_ix,
            &mut self.words,
            key,
        )
        .0
    }

    /// File name `name`'s load id, if it has one ([`super::Fam::Load`]).
    pub(super) fn load_ix(&self, name: &[u8]) -> Option<u32> {
        if let Some(b) = self.base.as_deref()
            && let Some(&i) = b.loads_ix.get(name)
        {
            return Some(i);
        }
        self.loads_ix.get(name).copied()
    }

    /// File name `name`'s load id, made (with version `v` and kind
    /// `kind`) if it has none; whether it was made.
    pub(super) fn load_id(
        &mut self,
        name: &[u8],
        v: Version,
        kind: crate::host::FileKind,
    ) -> (u32, bool) {
        if let Some(i) = self.load_ix(name) {
            return (i, false);
        }
        let from = self.base.as_deref().map_or(0, |b| b.loads.len());
        let i = u32::try_from(from + self.loads.len()).unwrap_or(u32::MAX);
        self.loads_ix.insert(name.to_vec(), i);
        self.loads.push((name.to_vec(), v, kind));
        (i, true)
    }

    /// Load `id` now has contents of version `v`, looked up as `kind` (a
    /// worker's change of a base entry is its own: [`RecState::load_sets`]).
    pub(super) fn set_load(&mut self, id: u32, v: Version, kind: crate::host::FileKind) {
        let i = id as usize;
        let from = self.base.as_deref().map_or(0, |b| b.loads.len());
        if i >= from {
            if let Some(l) = self.loads.get_mut(i - from) {
                l.1 = v;
                l.2 = kind;
            }
        } else {
            self.load_sets.insert(id, (v, kind));
        }
    }

    /// Load `i`: its name, its contents' version and its kind.
    pub(super) fn load_at(&self, i: usize) -> Option<(&[u8], Version, crate::host::FileKind)> {
        let l = at_in(self.base.as_deref().map(|b| &b.loads[..]), &self.loads, i)?;
        let (v, k) = u32::try_from(i)
            .ok()
            .and_then(|i| self.load_sets.get(&i))
            .copied()
            .unwrap_or((l.1, l.2));
        Some((&l.0, v, k))
    }

    /// Name `i` ([`super::Fam::Name`]).
    pub(super) fn name_at(&self, i: usize) -> Option<&[u8]> {
        at_in(self.base.as_deref().map(|b| &b.names[..]), &self.names, i).map(|n| &n[..])
    }

    /// Search `i` ([`super::Fam::Search`]).
    pub(super) fn search_at(&self, i: usize) -> Option<&[u8]> {
        at_in(
            self.base.as_deref().map(|b| &b.searches[..]),
            &self.searches,
            i,
        )
        .map(|n| &n[..])
    }

    /// Word `i` ([`super::Fam::HyphWord`]).
    pub(super) fn word_at(&self, i: usize) -> Option<&Vec<u8>> {
        at_in(self.base.as_deref().map(|b| &b.words[..]), &self.words, i)
    }

    /// The codes of id `id` lines were tokenized under.
    pub(super) fn code(&self, id: usize) -> Option<&LineCodes> {
        at_in(self.base.as_deref().map(|b| &b.codes[..]), &self.codes, id)
    }

    /// The id of codes `c`, made if they have none (their content's
    /// version the key).
    pub(super) fn codes_id(&mut self, c: &LineCodes) -> u32 {
        let key = Version::of(c).0;
        if let Some(b) = self.base.as_deref()
            && let Some(&i) = b.codes_ix.get(&key)
        {
            return i;
        }
        if let Some(&i) = self.codes_ix.get(&key) {
            return i;
        }
        let from = self.base.as_deref().map_or(0, |b| b.codes.len());
        let i = u32::try_from(from + self.codes.len())
            .unwrap_or(super::EOF - 1)
            .min(super::EOF - 1);
        self.codes.push(c.clone());
        self.codes_ix.insert(key, i);
        i
    }

    /// Whether the build opened file `name` for writing.
    pub(super) fn is_written(&self, name: &[u8]) -> bool {
        self.written.contains(name)
            || self
                .base
                .as_deref()
                .is_some_and(|b| b.written.contains(name))
    }
}

/// What a worker's run asked of the host that the build must see happen
/// at the step's commit, in order (the host's progress reports and the
/// viewer's streams, the terminal, the files closed).
pub(crate) enum HostEvent {
    Close(crate::host::WriteId),
    Term(Vec<u8>),
    Diagnostic(crate::diag::Diagnostic),
    Shipping(i32),
    Stream(Option<usize>, crate::pagepdf::ShippedStream),
    RemoveOutput(Vec<u8>),
}

/// What a round's workers read of the build, the same for each: where
/// the fonts were made, the names the build stores, the names it entered
/// and their makers, and the round's stop.
pub(super) struct Snapshot {
    pub(super) fonts: Arc<Vec<FontAt>>,
    pub(super) stored: Arc<BTreeSet<u32>>,
    pub(super) made_at: Arc<Vec<u32>>,
    pub(super) made: Arc<Vec<(u64, u32)>>,
    pub(super) stop: Arc<AtomicBool>,
}

impl Snapshot {
    /// The build's, now.
    pub(super) fn take(t: &SsaTracker, rr: &Recorder) -> Snapshot {
        Snapshot {
            fonts: Arc::new(fonts_at(t, rr)),
            stored: Arc::new(rr.st.steps.stored_ids()),
            made_at: Arc::new(t.made_at.borrow().clone()),
            made: Arc::new(t.made.borrow().clone()),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Each font slot's [`FontAt`], as the build's fold has it now (what
/// `font_made_by_now` answers for a step that did not make it).
pub(super) fn fonts_at(t: &SsaTracker, rr: &Recorder) -> Vec<FontAt> {
    let steps = &rr.rt.fold.steps;
    t.fonts_by
        .borrow()
        .iter()
        .map(|by| {
            let (sid, ser, n) = (*by)?;
            Some(match steps.get(sid as usize) {
                Some(m) => (m.key, n, m.serial == ser && m.live),
                None => (u64::MAX, n, false),
            })
        })
        .collect()
}

/// Whether the build's answers to what a worker's run asked of it (the
/// fonts' makers, the names' makers) are still the round's.
/// The fonts are compared by what a run asks of them, for the step at
/// `key` now and the run's then: whether each is visible
/// ([`Worker::font_visible`]) and its place among those made: keys are
/// made again as steps go into the fold (`Fold::renumber`), in the same
/// order.
pub(super) fn answers_hold(t: &SsaTracker, rr: &Recorder, w: &Worker, key: u64) -> bool {
    if w.asked_fonts.get() {
        let now = fonts_at(t, rr);
        let seen = |x: &FontAt, k: u64| x.is_none_or(|(m, _, live)| live && m < k);
        let place = |x: &FontAt| x.map(|(_, n, _)| n);
        if now.len() != w.fonts.len()
            || now
                .iter()
                .zip(w.fonts.iter())
                .any(|(a, b)| seen(a, key) != seen(b, w.key.get()) || place(a) != place(b))
        {
            return false;
        }
    }
    // (a name's maker, by what `SsaTracker::name_defined` does with it:
    // the name it makes where a step after the one asking entered it)
    let at = t.made_at.borrow();
    let made = t.made.borrow();
    w.asked_made.borrow().iter().all(|&p| {
        let Ok(i) = usize::try_from(p) else {
            return true;
        };
        let makes = |at: &[u32], made: &[(u64, u32)], k: u64| {
            at.get(i)
                .copied()
                .filter(|&m| m > 0)
                .and_then(|m| made.get(m as usize - 1))
                .filter(|&&(mk, _)| k != 0 && k < mk)
                .map(|&(_, id)| id)
        };
        makes(&at, &made, key) == makes(&w.made_at, &w.made, w.key.get())
    })
}

/// A worker's tracker made ready for a run of the step at `key`, salt
/// `salt`, from the build's recorder `rr` (its versions' revisions, its
/// data's ids) and the round's snapshot: no records, its tables empty
/// (read through the build's, [`Base`], set once the round lends them).
pub(super) fn prepare(
    shell: &mut SsaTracker,
    main: &SsaTracker,
    rr: &Recorder,
    snap: &Snapshot,
    key: u64,
    salt: u32,
) {
    shell.check = false;
    shell.lean = main.lean;
    shell.names_by_name = main.names_by_name;
    shell.timed = false;
    shell.apply.set(false);
    shell.stop_after.set(u64::MAX);
    shell.budget.set(u64::MAX);
    shell.deadline.set(None);
    shell.cancel.set(None);
    shell.keep_complete.set(main.keep_complete.get());
    shell.expanding.set((0, 0));
    shell.entry_saves.borrow_mut().clear();
    shell.undone.borrow_mut().clear();
    shell.calls.borrow_mut().clear();
    shell.worker = Some(alloc::boxed::Box::new(Worker::new(key, salt, snap)));
    // (every stamp of an earlier run stale)
    shell.boundary();
    let mut r = shell.rec.borrow_mut();
    r.rt.reset_records();
    r.rt.open_trip(1);
    r.rt.keep_step_versions(true);
    r.st = RecState {
        vers: super::Versions::worker_of(&rr.st.vers),
        generation: rr.st.generation,
        steps: super::rebuild::Steps::worker_of(&rr.st.steps),
        ..RecState::default()
    };
    r.on = true;
}

/// A step's run on a worker as it ended ([`Done`]): what it recorded,
/// where it left the input, its logs, its commands, and the classes of
/// the meanings it read the class of and not the meaning (each with the
/// class it read).
pub(super) struct Ran {
    pub(super) export: partex_ssa::export::StepExport<TexSsa>,
    pub(super) end: super::rebuild::InputState,
    pub(super) logs: super::rebuild::StepLogs,
    pub(super) commands: u64,
    pub(super) classes: Vec<(i32, u8)>,
    /// The pool's and the hash's allocators when it began.
    pub(super) str_ptr: usize,
    pub(super) hash_used: i32,
    pub(super) hash_high: i32,
    /// How far it moved the catcode generation.
    pub(super) generation: u64,
}

/// Why a worker's run cannot be taken, if it cannot: its tracker's taint
/// or its host's.
#[cfg(feature = "std")]
fn taint_of(tex: &Tex<WorkerHost, SsaTracker>) -> Option<&'static str> {
    tex.tracker
        .worker
        .as_ref()
        .and_then(|w| w.tainted.get())
        .or(tex.host.tainted)
}

/// Run step `j` on a worker's view `tex`, placed at `vals`, from `input`
/// (DESIGN 3.10, "Per worker"): as `rebuild::run_step` runs its first
/// try, the step open in the worker's own fold, ended at the clean point
/// that ends it there, its run exported.
#[cfg(feature = "std")]
fn run_on(
    tex: &mut Tex<WorkerHost, SsaTracker>,
    vals: &[(Slot, super::SVal)],
    input: &super::rebuild::InputState,
    budget: u64,
) -> Result<Ran, &'static str> {
    // (the view's flat parts made flat: the buffer, the pool)
    tex.thaw();
    super::rebuild::put(tex, vals);
    input.set(tex);
    tex.at_checkpoint = true;
    if let Some(e) = tex.effects.as_mut() {
        e.clear();
    }
    tex.log_file.buf.clear();
    for f in &mut tex.write_file {
        f.buf.clear();
    }
    run_here(tex, budget)
}

/// A step run on a worker's view where its input and state now are,
/// ended at the clean point that ends it ([`run_on`], [`run_chunk`]).
#[cfg(feature = "std")]
fn run_here(tex: &mut Tex<WorkerHost, SsaTracker>, budget: u64) -> Result<Ran, &'static str> {
    // (the meanings as the run begins, shared: a meaning whose class it
    // read and which it wrote is checked as it found it)
    let entry = tex.eqtb.clone();
    let (str_ptr, hash_used, hash_high) = (tex.str_ptr, tex.hash_used, tex.hash_high);
    let g0 = tex.tracker.rec.borrow().st.generation;
    let c0 = tex.commands();
    let mut srep = super::SsaReport::default();
    let mut open = Some(super::open_paragraph(tex, false, &mut srep, None));
    tex.tracker
        .stop_after
        .set(tex.commands().saturating_add(budget));
    let mut step = tex.resume();
    let fin = loop {
        match step {
            Step::Checkpoint => {
                if taint_of(tex).is_some()
                    || tex.tracker.worker.as_ref().is_some_and(|w| w.cancelled())
                    || super::step_ends(tex)
                {
                    break false;
                }
                step = tex.resume();
            }
            Step::Finished(_) => break true,
        }
    };
    tex.tracker.stop_after.set(u64::MAX);
    let cut = taint_of(tex).is_some();
    if fin || cut {
        tex.tracker.end_open_calls(&*tex);
    }
    super::close_paragraph(tex, &mut open, &mut srep, super::Close::Call);
    if let Some(why) = taint_of(tex) {
        return Err(why);
    }
    let end = super::rebuild::InputState::of(tex, fin);
    let export = {
        let mut r = tex.tracker.rec.borrow_mut();
        let (read, skip) = tex.tracker.step_end_softs(&mut r, tex.save_ptr);
        r.rt.export_step(&read, skip)
    }
    .ok_or("no step open")?;
    let logs = tex.tracker.rec.borrow_mut().st.steps.take_logs();
    // (a meaning's class read alone is a read of the meaning: the class
    // it found, if the run did not write the meaning)
    let reads: BTreeSet<Slot> = export.versioned().map(|(a, _)| *a).collect();
    let wrote: BTreeSet<Slot> = export.writes().into_iter().map(|(a, _)| a).collect();
    let mut classes = Vec::new();
    for a in reads.iter().filter(|a| a.0 == Fam::Class) {
        let e = Slot(Fam::Eqtb, a.1);
        if reads.contains(&e) {
            continue;
        }
        let p = i32::try_from(a.1).unwrap_or(0);
        let class = match usize::try_from(p) {
            Ok(i) if wrote.contains(&e) && p < crate::xregs::EXT_BASE && i < entry.len() => {
                let w = entry[i];
                crate::skipcache::token_class(w.b0(), w.rh())
            }
            _ if wrote.contains(&e) => return Err("a meaning's class read, the meaning written"),
            _ => tex.token_class_of(p),
        };
        classes.push((p, class));
    }
    Ok(Ran {
        export,
        end,
        logs,
        commands: tex.commands() - c0,
        classes,
        str_ptr,
        hash_used,
        hash_high,
        generation: tex.tracker.rec.borrow().st.generation - g0,
    })
}

/// A step run on a worker for a round ([`round`]): its place in the
/// round, the step, the worker's view of the engine, the values it is
/// placed at, where it begins, and its budget of commands.
#[cfg(feature = "std")]
pub(super) struct Job {
    pub(super) idx: usize,
    pub(super) j: StepId,
    pub(super) tex: Tex<WorkerHost, SsaTracker>,
    pub(super) vals: Vec<(Slot, super::SVal)>,
    pub(super) input: super::rebuild::InputState,
    pub(super) budget: u64,
}

/// A job's end: its step, the worker's tracker (its tables, its
/// answers: none if the run panicked) and its host's events, and its
/// run, or why it cannot be taken. The view itself is let go on the
/// worker's thread.
#[cfg(feature = "std")]
pub(super) struct Done {
    pub(super) j: StepId,
    pub(super) tracker: Option<alloc::boxed::Box<SsaTracker>>,
    pub(super) events: Vec<HostEvent>,
    pub(super) ran: Result<Ran, &'static str>,
}

/// A worker's view let go, its tracker and its host's events kept.
#[cfg(feature = "std")]
fn let_go(mut tex: Tex<WorkerHost, SsaTracker>) -> (alloc::boxed::Box<SsaTracker>, Vec<HostEvent>) {
    let t = core::mem::replace(&mut tex.tracker, SsaTracker::new(Recorder::new()));
    let events = core::mem::take(&mut tex.host.events);
    drop(tex);
    (alloc::boxed::Box::new(t), events)
}

/// What a worker's run asks of the build's host, which the build's thread
/// answers while it waits for the round ([`round`]).
#[cfg(feature = "std")]
pub(super) enum Ask {
    Read(Vec<u8>, crate::host::FileKind),
    Deflate(i32, Vec<u8>),
    Cached(u128),
    Cache(u128, crate::host::Memo),
    OutputName(Vec<u8>, crate::host::FileKind),
    WrittenName(Vec<u8>),
    /// A page a cold build's chunk shipped, shown provisionally (DESIGN
    /// 4.8's viewer): its step's commit shows it again, as built.
    Shipped(Option<usize>, crate::pagepdf::ShippedStream),
}

/// The build's answer to an [`Ask`].
#[cfg(feature = "std")]
pub(super) enum Reply {
    File(Option<crate::host::OpenedFile>),
    Bytes(Option<Vec<u8>>),
    Memo(Option<crate::host::Memo>),
    Name(Vec<u8>),
}

/// A message from a round's worker to the build's thread.
#[cfg(feature = "std")]
pub(super) enum Msg {
    Ask(usize, Ask),
    Done(usize, alloc::boxed::Box<dyn core::any::Any + Send>),
}

/// A worker's host: files read and streams compressed by the build's
/// host, asked through the build's thread; what the build must see
/// happen (the terminal, the progress reports, the viewer's streams)
/// kept for the step's commit; the rest (a file opened for writing, a
/// command, the terminal read) taints the run.
#[cfg(feature = "std")]
pub(super) struct WorkerHost {
    pub(super) job: usize,
    pub(super) ask: std::sync::mpsc::Sender<Msg>,
    pub(super) reply: std::sync::mpsc::Receiver<Reply>,
    pub(super) now: crate::host::DateTime,
    pub(super) notes: bool,
    pub(super) streams: bool,
    pub(super) commands: bool,
    pub(super) events: Vec<HostEvent>,
    pub(super) tainted: Option<&'static str>,
    /// A cold build's chunk: its pages shown at once, provisionally.
    pub(super) provisional: bool,
}

#[cfg(feature = "std")]
impl WorkerHost {
    fn taint(&mut self, why: &'static str) {
        if self.tainted.is_none() {
            self.tainted = Some(why);
        }
    }

    fn asked(&mut self, a: Ask) -> Option<Reply> {
        self.ask.send(Msg::Ask(self.job, a)).ok()?;
        self.reply.recv().ok()
    }
}

#[cfg(feature = "std")]
impl crate::host::Host for WorkerHost {
    fn read_file(
        &mut self,
        name: &[u8],
        kind: crate::host::FileKind,
    ) -> Option<crate::host::OpenedFile> {
        match self.asked(Ask::Read(name.to_vec(), kind)) {
            Some(Reply::File(f)) => f,
            _ => {
                self.taint("the host gone");
                None
            }
        }
    }
    fn open_write(
        &mut self,
        _name: &[u8],
        _kind: crate::host::FileKind,
    ) -> Option<(crate::host::WriteId, Vec<u8>)> {
        self.taint("a file opened for writing");
        None
    }
    fn open_write_again(
        &mut self,
        _name: &[u8],
        _kind: crate::host::FileKind,
        _id: crate::host::WriteId,
    ) -> Option<(crate::host::WriteId, Vec<u8>)> {
        self.taint("a file opened for writing");
        None
    }
    fn open_write_later(
        &mut self,
        _name: &[u8],
        _kind: crate::host::FileKind,
        _again: Option<crate::host::WriteId>,
    ) -> Option<(crate::host::WriteId, Vec<u8>)> {
        self.taint("a file opened for writing");
        None
    }
    fn written_name(&mut self, name: &[u8]) -> Vec<u8> {
        match self.asked(Ask::WrittenName(name.to_vec())) {
            Some(Reply::Name(n)) => n,
            _ => {
                self.taint("the host gone");
                name.to_vec()
            }
        }
    }
    fn output_edited(&mut self, _name: &[u8]) -> bool {
        self.taint("an output's edit asked");
        true
    }
    fn write(&mut self, _file: crate::host::WriteId, _bytes: &[u8]) {
        self.taint("a write to the host");
    }
    fn close(&mut self, _file: crate::host::WriteId) {
        self.taint("a file closed");
    }
    fn close_pipe(&mut self, _file: crate::host::WriteId) -> i32 {
        self.taint("a pipe closed");
        0
    }
    fn term_write(&mut self, bytes: &[u8]) {
        self.events.push(HostEvent::Term(bytes.to_vec()));
    }
    fn term_read_line(&mut self) -> Option<Vec<u8>> {
        self.taint("the terminal read");
        None
    }
    fn now(&self) -> crate::host::DateTime {
        self.now
    }
    fn creation_date(&mut self) -> Vec<u8> {
        self.taint("the creation date asked");
        Vec::new()
    }
    fn file_mod_date(&mut self, _name: &[u8]) -> Option<Vec<u8>> {
        self.taint("a file's date asked");
        None
    }
    fn seconds_and_micros(&mut self) -> (i32, i32) {
        self.taint("the clock asked");
        (0, 0)
    }
    fn diagnostic(&mut self, d: &crate::diag::Diagnostic) {
        self.events.push(HostEvent::Diagnostic(d.clone()));
    }
    fn notes(&self) -> bool {
        self.notes
    }
    fn shipping(&mut self, count0: i32) {
        self.events.push(HostEvent::Shipping(count0));
    }
    fn page_written(&mut self, _page: &partex_engine::pageir::Page) {
        self.taint("a DVI page written");
    }
    fn wants_streams(&self) -> bool {
        self.streams
    }
    fn stream_shipped(&mut self, page: Option<usize>, stream: crate::pagepdf::ShippedStream) {
        if self.provisional {
            let _ = self
                .ask
                .send(Msg::Ask(self.job, Ask::Shipped(page, stream.clone())));
        }
        self.events.push(HostEvent::Stream(page, stream));
    }
    fn deflate(&mut self, level: i32, data: &[u8]) -> Option<Vec<u8>> {
        match self.asked(Ask::Deflate(level, data.to_vec())) {
            Some(Reply::Bytes(b)) => b,
            _ => {
                self.taint("the host gone");
                None
            }
        }
    }
    fn cached(&mut self, key: u128) -> Option<crate::host::Memo> {
        match self.asked(Ask::Cached(key)) {
            Some(Reply::Memo(m)) => m,
            _ => None,
        }
    }
    fn cache(&mut self, key: u128, value: crate::host::Memo) {
        let _ = self.ask.send(Msg::Ask(self.job, Ask::Cache(key, value)));
    }
    fn font_slot(&mut self, _ident: u128, fresh: i32) -> i32 {
        self.taint("a font loaded");
        fresh
    }
    fn system(
        &mut self,
        _command: &[u8],
        _inputs: &[(Vec<u8>, Arc<[u8]>)],
    ) -> Option<crate::host::Ran> {
        self.taint("a command run");
        None
    }
    fn out_name_ok(&mut self, _name: &[u8]) -> bool {
        self.taint("a file opened for writing");
        true
    }
    fn runs_commands(&self) -> bool {
        self.commands
    }
    fn synctex_name(&mut self, found: &[u8]) -> Vec<u8> {
        self.taint("SyncTeX");
        found.to_vec()
    }
    fn remove_output(&mut self, name: &[u8]) {
        self.events.push(HostEvent::RemoveOutput(name.to_vec()));
    }
    fn output_name(&mut self, name: &[u8], kind: crate::host::FileKind) -> Vec<u8> {
        match self.asked(Ask::OutputName(name.to_vec(), kind)) {
            Some(Reply::Name(n)) => n,
            _ => {
                self.taint("the host gone");
                name.to_vec()
            }
        }
    }
}

/// The build's host answering a worker's [`Ask`] (none for a
/// [`Ask::Cache`], which wants no answer).
#[cfg(feature = "std")]
fn serve<H: crate::host::Host>(host: &mut H, a: Ask) -> Option<Reply> {
    Some(match a {
        Ask::Read(n, k) => Reply::File(host.read_file(&n, k)),
        Ask::Deflate(l, d) => Reply::Bytes(host.deflate(l, &d)),
        Ask::Cached(k) => Reply::Memo(host.cached(k)),
        Ask::Cache(k, v) => {
            host.cache(k, v);
            return None;
        }
        Ask::OutputName(n, k) => Reply::Name(host.output_name(&n, k)),
        Ask::WrittenName(n) => Reply::Name(host.written_name(&n)),
        Ask::Shipped(p, s) => {
            host.stream_shipped(p, s);
            return None;
        }
    })
}

/// The workers' threads' name.
#[cfg(feature = "std")]
const WORKER: &str = "phitex-ssa-worker";

/// A worker's run that panics (a state it was wrongly given can lead the
/// engine where no run of the build goes) is not taken, and says
/// nothing: the panic hook passes over the workers' threads, and every
/// other thread's panics to the hook it had.
#[cfg(feature = "std")]
fn quiet_workers() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let was = std::panic::take_hook();
        // (`PHITEX_SSA_WORKER_PANICS=1`: said, to debug)
        let say = std::env::var_os("PHITEX_SSA_WORKER_PANICS").is_some();
        std::panic::set_hook(alloc::boxed::Box::new(move |info| {
            if say || std::thread::current().name() != Some(WORKER) {
                was(info);
            }
        }));
    });
}

/// A worker thread's stack: the engine's recursion (expansion, boxes in
/// boxes) as deep as on the build's thread.
#[cfg(feature = "std")]
const STACK: usize = 32 << 20;

/// Run `jobs` on `threads` threads (DESIGN 3.10, "Threads"), the build's
/// thread answering their asks of its host meanwhile (`rx` their
/// messages, `replies` each job's answers): each job's end, in the
/// jobs' order.
#[cfg(feature = "std")]
pub(super) fn round<H: crate::host::Host>(
    host: &mut H,
    jobs: Vec<Job>,
    threads: usize,
    tx: std::sync::mpsc::Sender<Msg>,
    rx: &std::sync::mpsc::Receiver<Msg>,
    replies: &[std::sync::mpsc::Sender<Reply>],
) -> Vec<Option<Done>> {
    let jobs = jobs.into_iter().map(|j| (j.idx, j)).collect();
    run_threads(
        host,
        jobs,
        threads,
        tx,
        rx,
        replies,
        |job: Job| {
            let Job {
                j,
                mut tex,
                vals,
                input,
                budget,
                ..
            } = job;
            let ran = run_on(&mut tex, &vals, &input, budget);
            let (tracker, events) = let_go(tex);
            Done {
                j,
                tracker: Some(tracker),
                events,
                ran,
            }
        },
        |job: &Job| (job.j, ()),
        |(j, ()): (StepId, ())| Done {
            j,
            tracker: None,
            events: Vec::new(),
            ran: Err("a panic"),
        },
        &mut || {},
    )
}

/// Run `jobs` (each with its place in the round) on `threads` threads,
/// each by `run`, the build's thread answering their asks of its host
/// meanwhile; each job's end in the jobs' order, `lost` of what `keep`
/// kept of a job whose run panicked.
#[cfg(feature = "std")]
#[allow(clippy::too_many_arguments)]
fn run_threads<H: crate::host::Host, J: Send, D: Send + 'static, K: Send>(
    host: &mut H,
    jobs: Vec<(usize, J)>,
    threads: usize,
    tx: std::sync::mpsc::Sender<Msg>,
    rx: &std::sync::mpsc::Receiver<Msg>,
    replies: &[std::sync::mpsc::Sender<Reply>],
    run: fn(J) -> D,
    keep: fn(&J) -> K,
    lost: fn(K) -> D,
    on_ship: &mut dyn FnMut(),
) -> Vec<Option<D>> {
    let n = jobs.len();
    quiet_workers();
    let mut out: Vec<Option<D>> = (0..n).map(|_| None).collect();
    let queue = std::sync::Mutex::new(jobs.into_iter().rev().collect::<Vec<(usize, J)>>());
    std::thread::scope(|sc| {
        let mut started = 0usize;
        for _ in 0..threads.clamp(1, n.max(1)) {
            let q = &queue;
            let tx = tx.clone();
            let spawned = std::thread::Builder::new()
                .name(alloc::string::String::from(WORKER))
                .stack_size(STACK)
                .spawn_scoped(sc, move || {
                    loop {
                        let job = q
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .pop();
                        let Some((idx, job)) = job else { break };
                        let k = keep(&job);
                        let d = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(job)))
                            .unwrap_or_else(|_| lost(k));
                        if tx.send(Msg::Done(idx, alloc::boxed::Box::new(d))).is_err() {
                            break;
                        }
                    }
                });
            if spawned.is_err() {
                break;
            }
            started += 1;
        }
        drop(tx);
        let mut left = n;
        if started == 0 {
            // (no thread: each step runs at its turn)
            for (idx, job) in queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .drain(..)
            {
                if let Some(o) = out.get_mut(idx) {
                    *o = Some(lost(keep(&job)));
                }
            }
            left = 0;
        }
        while left > 0 {
            match rx.recv() {
                Ok(Msg::Ask(i, a)) => {
                    if matches!(a, Ask::Shipped(..)) {
                        on_ship();
                    }
                    if let Some(r) = serve(host, a)
                        && let Some(s) = replies.get(i)
                    {
                        let _ = s.send(r);
                    }
                }
                Ok(Msg::Done(i, d)) => {
                    if let (Some(o), Ok(d)) = (out.get_mut(i), d.downcast::<D>()) {
                        *o = Some(*d);
                    }
                    left -= 1;
                }
                Err(_) => break,
            }
        }
    });
    out
}

/// A step of a cold build's chunk, run on a worker (`cold.rs`): where it
/// began, its run, its salt, and what its tracker and its host kept of
/// it (the names it entered, the loads it found otherwise, the files it
/// opened for writing, the host's events).
#[cfg(feature = "std")]
pub(super) struct ChunkStep {
    pub(super) start: super::rebuild::InputState,
    /// The count registers where it began.
    pub(super) counts: Vec<i32>,
    pub(super) ran: Ran,
    pub(super) salt: u32,
    pub(super) made: Vec<(i32, u32)>,
    pub(super) load_sets: BTreeMap<u32, (Version, crate::host::FileKind)>,
    pub(super) written: BTreeSet<Vec<u8>>,
    pub(super) events: Vec<HostEvent>,
}

/// A cold build's chunk for a worker (`cold.rs`): the worker's view, the
/// count registers set on it (eqtb's place, the value), where it begins,
/// the main file and its line past which a step's end ends the chunk,
/// the build's counter of salts, and the most steps it runs.
#[cfg(feature = "std")]
pub(super) struct ChunkJob {
    pub(super) tex: Tex<WorkerHost, SsaTracker>,
    pub(super) fix: Vec<(i32, i32)>,
    pub(super) input: super::rebuild::InputState,
    pub(super) main: Arc<[u8]>,
    pub(super) stop: i32,
    pub(super) salts: Arc<core::sync::atomic::AtomicU32>,
    pub(super) max: usize,
    /// The base's last font: those after it the run loaded.
    pub(super) fonts_from: i32,
}

/// A chunk's run: the worker's tracker (none if it panicked), its steps
/// in order, why it stopped before its end (if it did), and the count
/// registers where it began and where it stopped.
#[cfg(feature = "std")]
pub(super) struct ChunkDone {
    pub(super) tracker: Option<alloc::boxed::Box<SsaTracker>>,
    pub(super) steps: Vec<ChunkStep>,
    pub(super) why: Option<&'static str>,
    pub(super) entry: Vec<i32>,
    pub(super) exit: Vec<i32>,
    /// The TFM fonts it loaded, if a font it loaded stopped it: each one's
    /// name, area and size (`Tex::preload_font`).
    pub(super) fonts: Vec<(Vec<u8>, Vec<u8>, i32)>,
}

/// The count registers 0 to 255 of `tex`.
pub(super) fn counts<H: crate::host::Host, T: crate::track::Tracker>(tex: &Tex<H, T>) -> Vec<i32> {
    (0..256)
        .map(|r| tex.peek_eqtb(crate::web::COUNT_BASE + r).int())
        .collect()
}

/// Run a chunk (DESIGN 3.10, "Cold builds"): from its start, step after
/// step, each exported, until a step ends at or past the chunk's last
/// line, the job ends, or a run cannot be taken.
#[cfg(feature = "std")]
fn run_chunk(job: ChunkJob) -> ChunkDone {
    let ChunkJob {
        mut tex,
        fix,
        input,
        main,
        stop,
        salts,
        max,
        fonts_from,
    } = job;
    tex.thaw();
    for (p, v) in fix {
        let Ok(i) = usize::try_from(p) else { continue };
        let mut w = tex.eqtb[i];
        w.set_int(v);
        tex.eqtb[i] = w;
        tex.tracker
            .rec
            .borrow_mut()
            .st
            .vers
            .set_stale(Slot(Fam::Eqtb, i64::from(p)));
    }
    input.set(&mut tex);
    tex.at_checkpoint = true;
    if let Some(e) = tex.effects.as_mut() {
        e.clear();
    }
    tex.log_file.buf.clear();
    for f in &mut tex.write_file {
        f.buf.clear();
    }
    let entry = counts(&tex);
    let mut at = entry.clone();
    let mut steps = Vec::new();
    let mut start = input;
    let mut why = None;
    while steps.len() < max {
        let salt = 0x8000_0000 | (salts.fetch_add(1, Ordering::Relaxed) & 0x7fff_ffff);
        if let Some(w) = tex.tracker.worker.as_deref() {
            w.salt.set(salt);
        }
        match run_here(&mut tex, u64::MAX >> 2) {
            Ok(ran) => {
                let end = ran.end.clone();
                let made = tex
                    .tracker
                    .worker
                    .as_deref()
                    .map(|w| core::mem::take(&mut *w.made_names.borrow_mut()))
                    .unwrap_or_default();
                let (load_sets, written) = {
                    let mut r = tex.tracker.rec.borrow_mut();
                    (
                        core::mem::take(&mut r.st.load_sets),
                        r.st.written.clone(),
                    )
                };
                let events = core::mem::take(&mut tex.host.events);
                steps.push(ChunkStep {
                    start,
                    counts: at,
                    ran,
                    salt,
                    made,
                    load_sets,
                    written,
                    events,
                });
                if end.finished() || end.ends_line(&main, stop) {
                    break;
                }
                start = end;
                at = counts(&tex);
            }
            Err(e) => {
                why = Some(e);
                break;
            }
        }
    }
    let exit = counts(&tex);
    // (the fonts it loaded past the base's, for the build to load first)
    let fonts = if why == Some("a font loaded") && !tex.unicode {
        (fonts_from + 1..=tex.font_ptr)
            .map(|f| {
                let i = crate::fonts::fx(f);
                let bytes = |s: i32| tex.str_bytes(usize::try_from(s).unwrap_or(0)).to_vec();
                (
                    bytes(tex.fonts.name[i]),
                    bytes(tex.fonts.area[i]),
                    tex.fonts.get(f).size,
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    let (tracker, _) = let_go(tex);
    ChunkDone {
        tracker: Some(tracker),
        steps,
        why,
        entry,
        exit,
        fonts,
    }
}

/// Run a cold build's chunks on `threads` threads ([`run_threads`]),
/// their pages shown provisionally as they ship (`on_ship` told).
#[cfg(feature = "std")]
pub(super) fn chunk_round<H: crate::host::Host>(
    host: &mut H,
    jobs: Vec<ChunkJob>,
    threads: usize,
    tx: std::sync::mpsc::Sender<Msg>,
    rx: &std::sync::mpsc::Receiver<Msg>,
    replies: &[std::sync::mpsc::Sender<Reply>],
    on_ship: &mut dyn FnMut(),
) -> Vec<Option<ChunkDone>> {
    let jobs = jobs.into_iter().enumerate().collect();
    run_threads(
        host,
        jobs,
        threads,
        tx,
        rx,
        replies,
        run_chunk,
        |_: &ChunkJob| (),
        |()| ChunkDone {
            tracker: None,
            steps: Vec::new(),
            why: Some("a panic"),
            entry: Vec::new(),
            exit: Vec::new(),
            fonts: Vec::new(),
        },
        on_ship,
    )
}

/// Replay a committed run's host events on the build's host.
pub(super) fn replay_events<H: crate::host::Host>(host: &mut H, events: Vec<HostEvent>) {
    for e in events {
        match e {
            HostEvent::Close(id) => host.close(id),
            HostEvent::Term(b) => host.term_write(&b),
            HostEvent::Diagnostic(d) => host.diagnostic(&d),
            HostEvent::Shipping(c) => host.shipping(c),
            HostEvent::Stream(p, s) => host.stream_shipped(p, s),
            HostEvent::RemoveOutput(n) => host.remove_output(&n),
        }
    }
}
