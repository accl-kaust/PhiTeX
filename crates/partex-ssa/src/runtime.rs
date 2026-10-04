//! Evaluation (`DESIGN.md` §7.17.2, §7.17.3, §7.17.5): records, the
//! memo by name, reads verified in order, and the trips of a build.
//!
//! A call's record holds its name, its body's *own* reads in the order
//! they happened (7.17.2: a read of a slot the call wrote before, itself
//! or through a child, is inside the call and not recorded; a child's
//! reads stay in the child's record), its net writes (its children's
//! included), its effects and children in program order with the first
//! write of each slot its own body made among them ([`Item::Wrote`]),
//! and its cost. A record found by name is a hit iff the reads of its
//! subtree from outside it hold the versions now current: its own reads,
//! then each child's, a child's read of a slot the call or an earlier
//! child wrote before it being inside the call ([`Runtime::verify`]
//! walks them). A hit puts the writes in place and its children are not
//! run; the first read that differs is a miss, and the body runs under a
//! frame that records, each nested call evaluated by the same rule.
//!
//! Reads inside a frame are told from its own by serials, not sets:
//! every read and write takes the next serial, a frame remembers the
//! serial it began at, and each slot remembers the serial of its last
//! write. A read is the frame's own iff the slot's last write is older
//! than the frame.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::hash::{Stable, Version, hash64};
use crate::machine::{Cx, Loc, Machine, Stream};
use crate::open::{MapStore, Store};
use crate::pmap::PMap;
use crate::pvec::PVec;
use crate::table::Table;
use crate::value::{Value, version_opt};

/// A record's index in the arena.
pub type RecId = u32;

/// An effect or a child, in program order.
pub enum Item<M: Machine + ?Sized> {
    Out(M::Effect),
    Open(M::Addr),
    Store(M::Addr, M::Val),
    Call(RecId),
    /// The call's own body wrote the slot here for the first time: a
    /// later child's read of it is inside the call ([`Runtime::verify`]).
    Wrote(M::Addr),
}

impl<M: Machine + ?Sized> Clone for Item<M> {
    fn clone(&self) -> Self {
        match self {
            Item::Out(e) => Item::Out(e.clone()),
            Item::Open(a) => Item::Open(a.clone()),
            Item::Store(a, v) => Item::Store(a.clone(), v.clone()),
            Item::Call(c) => Item::Call(*c),
            Item::Wrote(a) => Item::Wrote(a.clone()),
        }
    }
}

/// What an effect reaches, for 7.17.11's slicing: a trip before the
/// last needs only what reaches a store.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectKind {
    Store,
    Output,
}

/// A call's record.
pub struct Record<M: Machine + ?Sized> {
    pub func: M::Func,
    pub name: Version,
    pub args: Vec<Version>,
    pub result: M::Val,
    /// The body's own reads, in order (7.17.2).
    pub reads: Vec<(Loc<M::Addr>, Version)>,
    pub writes: Vec<(M::Addr, Option<M::Val>)>,
    pub items: Vec<Item<M>>,
    /// The cost of the subtree.
    pub cost: u64,
    /// The cost of this body alone.
    pub own: u64,
    /// The record's content version, its identity in the arena.
    pub content: Version,
}

impl<M: Machine + ?Sized> Record<M> {
    /// The children, in order.
    pub fn children(&self) -> impl Iterator<Item = RecId> + '_ {
        self.items.iter().filter_map(|i| match i {
            Item::Call(c) => Some(*c),
            _ => None,
        })
    }
}

/// The records by one name, in the order they were made: the first
/// inline, so a lookup of a name with one record reads no other memory.
pub(crate) struct Cands {
    first: RecId,
    more: Vec<RecId>,
}

impl Cands {
    fn one(id: RecId) -> Self {
        Cands {
            first: id,
            more: Vec::new(),
        }
    }

    fn push(&mut self, id: RecId) {
        self.more.push(id);
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = RecId> + '_ {
        core::iter::once(self.first).chain(self.more.iter().copied())
    }
}

/// The runtime's switches.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Record and reuse. Off, every call runs plainly, with identical
    /// output (§0, principle 4).
    pub record: bool,
    /// Builds whose records are kept (undo); older ones are collected.
    pub keep: usize,
    /// The trip bound (latexmk's).
    pub max_trips: usize,
    /// A clock in nanoseconds, for the per-trip times.
    pub clock: Option<fn() -> u64>,
    /// Called on every effect a body makes, with what it reaches.
    pub on_effect: Option<fn(EffectKind)>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            record: true,
            keep: 3,
            max_trips: 5,
            clock: None,
            on_effect: None,
        }
    }
}

/// Counters over the runtime's life.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Stats {
    pub hits: u64,
    pub misses: u64,
    /// Misses with no record by that name.
    pub fresh: u64,
    pub reads_verified: u64,
    /// The own cost of the bodies that ran.
    pub cost_rerun: u64,
    /// Calls run with recording off.
    pub plain_calls: u64,
    pub trips: u64,
    pub builds: u64,
    pub collected: u64,
}

/// How an evaluated call was found.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Status<A> {
    Hit,
    /// No record by that name.
    New,
    /// Records by that name, none verified; the first read that differed.
    Miss(Loc<A>),
}

/// A stream whose value changed in a trip.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StreamChange<A> {
    pub stream: A,
    pub lines: usize,
    /// Lines that differ from the φ, position by position.
    pub differing: usize,
    /// Whether anything in the trip loaded it.
    pub loaded: bool,
}

/// One trip of a build.
#[derive(Clone, Debug)]
pub struct TripReport<M: Machine + ?Sized> {
    pub index: usize,
    pub changed: Vec<StreamChange<M::Addr>>,
    /// The calls whose bodies ran, in program order.
    pub reran: Vec<(M::Func, Version)>,
    pub hits: u64,
    pub misses: u64,
    pub loaded: Vec<M::Addr>,
    pub converged: bool,
    pub nanos: u64,
}

/// A build's result.
pub struct Outcome<M: Machine + ?Sized> {
    /// The last trip's output effects, in program order.
    pub effects: Vec<M::Effect>,
    /// The last trip's streams.
    pub streams: BTreeMap<M::Addr, Stream<M::Val>>,
    pub trips: Vec<TripReport<M>>,
    pub converged: bool,
}

/// Sizes of the last trip, for the next one's allocations.
#[derive(Default)]
pub(crate) struct Sizes {
    pub(crate) reran: usize,
    pub(crate) effects: usize,
    /// Items of the last frame of a function with many, by its hash.
    pub(crate) items: Table<Version, usize>,
}

/// A trip as the trace shows it.
pub(crate) struct TripLog<M: Machine + ?Sized> {
    pub index: usize,
    pub phi: PMap<M::Addr, Stream<M::Val>>,
    pub root: Vec<Item<M>>,
    pub statuses: Vec<Status<M::Addr>>,
    pub changed: Vec<StreamChange<M::Addr>>,
    pub streams: BTreeMap<M::Addr, usize>,
}

/// The runtime: the arena of records, the memo, and the streams the
/// last build stored.
pub struct Runtime<M: Machine> {
    pub cfg: Config,
    pub(crate) recs: Vec<Option<Record<M>>>,
    free: Vec<RecId>,
    pub(crate) memo: Table<Version, Cands>,
    dedup: Table<Version, RecId>,
    roots: VecDeque<Vec<RecId>>,
    streams: PMap<M::Addr, Stream<M::Val>>,
    live: usize,
    live_after_gc: usize,
    pub stats: Stats,
    pub(crate) start_phi: PMap<M::Addr, Stream<M::Val>>,
    pub(crate) log: Vec<TripLog<M>>,
    /// The recorder of an open build (`open.rs`).
    pub(crate) open: crate::open::Open<M>,
    /// The build's top level as a fold of steps, with the definitions and
    /// readers of every slot (`fold.rs`, 7.17.3).
    pub fold: crate::fold::Fold<M>,
    /// Per record: its subtree has no effects or stores, so the link
    /// passes it without reading it.
    inert: Vec<bool>,
    /// Per record: it is never looked up (a lean frame's,
    /// [`Runtime::begin_lean`]: a step's), so a kept build's roots do not
    /// keep it, only its children ([`Runtime::collect`]).
    lean: Vec<bool>,
    /// The writes the records made since the last collection held, and
    /// the live records' after it ([`Runtime::keep_roots`]).
    writes_made: usize,
    writes_after_gc: usize,
    /// The last trip's counts, to size the next trip's vectors, and the
    /// items of each function's last large frame, by the function's hash.
    pub(crate) sizes: Sizes,
    /// The last build's status vector, reused.
    pub(crate) spare_statuses: Vec<Status<M::Addr>>,
    pub(crate) default_val: Option<M::Val>,
    /// Each step's last run's times, by step id, with timing on
    /// ([`Runtime::set_timing`]).
    pub(crate) step_times: Vec<Option<crate::open::StepTimes<M::Addr>>>,
}

fn stream_version<V: Value>(s: Option<&Stream<V>>) -> Version {
    s.map_or(Version::ABSENT, PVec::version)
}

struct TripOut<M: Machine> {
    effects: Vec<M::Effect>,
    streams: BTreeMap<M::Addr, Stream<M::Val>>,
    loaded: Vec<M::Addr>,
    reran: Vec<(M::Func, Version)>,
    hits: u64,
    misses: u64,
    root: Vec<Item<M>>,
    statuses: Vec<Status<M::Addr>>,
}

impl<M: Machine> Runtime<M> {
    #[must_use]
    pub fn new(cfg: Config) -> Self {
        Runtime {
            cfg,
            recs: Vec::new(),
            free: Vec::new(),
            memo: Table::new(),
            dedup: Table::new(),
            roots: VecDeque::new(),
            streams: PMap::new(),
            live: 0,
            live_after_gc: 0,
            stats: Stats::default(),
            start_phi: PMap::new(),
            log: Vec::new(),
            open: crate::open::Open::new(),
            fold: crate::fold::Fold::default(),
            inert: Vec::new(),
            lean: Vec::new(),
            writes_made: 0,
            writes_after_gc: 0,
            sizes: Sizes::default(),
            spare_statuses: Vec::new(),
            default_val: None,
            step_times: Vec::new(),
        }
    }

    /// Records alive in the arena.
    #[must_use]
    pub fn live_records(&self) -> usize {
        self.live
    }

    /// The live records (a report: `PARTEX_SSA_MEM`).
    pub fn records(&self) -> impl Iterator<Item = &Record<M>> {
        self.recs.iter().flatten()
    }

    #[must_use]
    pub fn record(&self, id: RecId) -> &Record<M> {
        self.recs[id as usize].as_ref().expect("a live record")
    }

    /// What the records and the fold hold, roughly, in bytes by part (a
    /// report: `PARTEX_SSA_MEM`; the values' own contents not counted).
    #[must_use]
    pub fn mem_report(&self) -> alloc::string::String {
        use core::mem::size_of;
        let (mut n, mut rc, mut wc, mut ic, mut ac, mut held) = (0usize, 0, 0, 0, 0, 0);
        let (mut rl, mut wl) = (0usize, 0);
        for r in self.recs.iter().flatten() {
            n += 1;
            rl += r.reads.len();
            rc += r.reads.capacity();
            wl += r.writes.len();
            wc += r.writes.capacity();
            ic += r.items.capacity();
            ac += r.args.capacity();
            held += r.writes.iter().filter(|w| w.1.is_some()).count();
        }
        let (sr, sw, si) = (
            size_of::<(Loc<M::Addr>, Version)>(),
            size_of::<(M::Addr, Option<M::Val>)>(),
            size_of::<Item<M>>(),
        );
        alloc::format!(
            "records: {n} of {} ({} B each, {} MB); reads {rl} (capacity {rc}, {sr} B: {} MB); writes {wl} (capacity {wc}, {sw} B: {} MB, {held} with a value); items capacity {ic} ({si} B: {} MB); arguments {} MB; memo {} names; {}",
            self.recs.len(),
            size_of::<Option<Record<M>>>(),
            (self.recs.capacity() * size_of::<Option<Record<M>>>()) >> 20,
            (rc * sr) >> 20,
            (wc * sw) >> 20,
            (ic * si) >> 20,
            (ac * size_of::<Version>()) >> 20,
            self.memo.len(),
            self.fold.mem_report()
        )
    }

    /// The streams the last build's last trip stored.
    #[must_use]
    pub fn streams(&self) -> &PMap<M::Addr, Stream<M::Val>> {
        &self.streams
    }

    /// Start the next build from these streams (a session's files).
    pub fn set_streams(&mut self, s: PMap<M::Addr, Stream<M::Val>>) {
        self.streams = s;
    }

    fn now(&self) -> u64 {
        self.cfg.clock.map_or(0, |c| c())
    }

    /// Build: evaluate `f(args)` in trips until every stream a trip
    /// loaded equals what it stored, at most `max_trips`.
    pub fn build(&mut self, m: &M, f: M::Func, args: &[M::Val]) -> Outcome<M> {
        self.stats.builds += 1;
        self.start_phi = self.streams.clone();
        self.clear_log();
        let mut phi = self.streams.clone();
        let mut reports = Vec::new();
        let mut build_roots = Vec::new();
        let mut index = 0;
        loop {
            let t0 = self.now();
            let out = if self.cfg.record {
                self.trip_recorded(m, f, args, &phi)
            } else {
                self.trip_plain(m, f, args, &phi)
            };
            self.stats.trips += 1;
            let mut changed = Vec::new();
            let mut names: Vec<&M::Addr> = phi.entries().into_iter().map(|(k, _)| k).collect();
            names.extend(out.streams.keys());
            names.sort();
            names.dedup();
            for s in names {
                let before = phi.get(s);
                let after = out.streams.get(s);
                if stream_version(before) != stream_version(after) {
                    let lines = after.map_or(0, PVec::len);
                    let empty = PVec::new();
                    let (b, a) = (before.unwrap_or(&empty), after.unwrap_or(&empty));
                    let differing = b
                        .iter()
                        .zip(a.iter())
                        .filter(|(x, y)| x.version() != y.version())
                        .count()
                        + b.len().abs_diff(a.len());
                    changed.push(StreamChange {
                        stream: s.clone(),
                        lines,
                        differing,
                        loaded: out.loaded.contains(s),
                    });
                }
            }
            let converged = changed.iter().all(|c| !c.loaded);
            for it in &out.root {
                if let Item::Call(id) = it {
                    build_roots.push(*id);
                }
            }
            reports.push(TripReport {
                index,
                changed: changed.clone(),
                reran: out.reran,
                hits: out.hits,
                misses: out.misses,
                loaded: out.loaded,
                converged,
                nanos: self.now().saturating_sub(t0),
            });
            let next: PMap<M::Addr, Stream<M::Val>> = {
                let mut p = PMap::new();
                for (k, v) in &out.streams {
                    p.insert(k.clone(), v.clone());
                }
                p
            };
            self.log.push(TripLog {
                index,
                phi: phi.clone(),
                root: out.root,
                statuses: out.statuses,
                changed,
                streams: out
                    .streams
                    .iter()
                    .map(|(k, v)| (k.clone(), v.len()))
                    .collect(),
            });
            index += 1;
            if converged || index >= self.cfg.max_trips.max(1) {
                self.streams = next;
                self.keep_roots(build_roots);
                return Outcome {
                    effects: out.effects,
                    streams: out.streams,
                    trips: reports,
                    converged,
                };
            }
            phi = next;
        }
    }

    fn trip_plain(
        &mut self,
        m: &M,
        f: M::Func,
        args: &[M::Val],
        phi: &PMap<M::Addr, Stream<M::Val>>,
    ) -> TripOut<M> {
        let mut cx = Plain::<M> {
            m,
            state: PMap::new(),
            phi,
            effects: Vec::new(),
            stores: BTreeMap::new(),
            loaded: Vec::new(),
            calls: 0,
            on_effect: self.cfg.on_effect,
        };
        let _ = cx.call(f, args);
        self.stats.plain_calls += cx.calls;
        TripOut {
            effects: cx.effects,
            streams: cx
                .stores
                .into_iter()
                .map(|(k, v)| (k, PVec::from_vec(v)))
                .collect(),
            loaded: cx.loaded,
            reran: Vec::new(),
            hits: 0,
            misses: 0,
            root: Vec::new(),
            statuses: Vec::new(),
        }
    }

    fn trip_recorded(
        &mut self,
        m: &M,
        f: M::Func,
        args: &[M::Val],
        phi: &PMap<M::Addr, Stream<M::Val>>,
    ) -> TripOut<M> {
        let spare = core::mem::take(&mut self.spare_statuses);
        self.open.reset(spare, self.sizes.reran);
        let mut st = MapStore {
            state: PMap::new(),
            phi: phi.clone(),
        };
        let h0 = self.stats;
        let _ = Eval {
            m,
            rt: self,
            st: &mut st,
        }
        .call(f, args);
        let root = core::mem::replace(&mut self.open.frames[0], Frame::root()).items;
        let statuses = core::mem::take(&mut self.open.statuses);
        let loaded = core::mem::take(&mut self.open.loaded);
        let reran = core::mem::take(&mut self.open.reran);
        let hits = self.stats.hits - h0.hits;
        let misses = self.stats.misses - h0.misses;
        let mut effects = Vec::with_capacity(self.sizes.effects);
        let mut stores: BTreeMap<M::Addr, Vec<M::Val>> = BTreeMap::new();
        self.flatten(&root, &mut effects, &mut stores);
        self.sizes.reran = reran.len();
        self.sizes.effects = effects.len();
        TripOut {
            effects,
            streams: stores
                .into_iter()
                .map(|(k, v)| (k, PVec::from_vec(v)))
                .collect(),
            loaded,
            reran,
            hits,
            misses,
            root,
            statuses,
        }
    }

    /// Drop the last build's trace, keeping its status vector's memory.
    pub(crate) fn clear_log(&mut self) {
        for l in self.log.drain(..) {
            if l.statuses.capacity() > self.spare_statuses.capacity() {
                self.spare_statuses = l.statuses;
            }
        }
        self.spare_statuses.clear();
    }

    /// Keep a build's roots (the last `keep` builds'), collecting when
    /// the arena has doubled.
    pub(crate) fn keep_roots(&mut self, roots: Vec<RecId>) {
        self.roots.push_back(roots);
        while self.roots.len() > self.cfg.keep.max(1) {
            self.roots.pop_front();
        }
        // (or when the writes the records hold grew by half: a step's
        // record holds its step's, a hundred where a routine's holds a few,
        // and a trip that runs most steps again, a cascade gone cold, makes
        // few records that hold as much as the build's)
        if self.live > 2 * self.live_after_gc + 4096
            || self.writes_made > self.writes_after_gc / 2 + (1 << 16)
        {
            self.collect();
        }
    }

    /// Record `id`, made now (`fresh`) or found equal to one made before,
    /// is a lean frame's (`lean`): it is lean if every call that made it
    /// was.
    pub(crate) fn note_lean(&mut self, id: RecId, fresh: bool, lean: bool) {
        let i = id as usize;
        if self.lean.len() <= i {
            self.lean.resize(i + 1, false);
        }
        self.lean[i] = lean && (fresh || self.lean[i]);
    }

    /// The effects and stores under `items`, in program order.
    pub(crate) fn flatten(
        &self,
        items: &[Item<M>],
        effects: &mut Vec<M::Effect>,
        stores: &mut BTreeMap<M::Addr, Vec<M::Val>>,
    ) {
        for it in items {
            match it {
                Item::Out(e) => effects.push(e.clone()),
                Item::Open(a) => {
                    stores.entry(a.clone()).or_default();
                }
                Item::Store(a, v) => stores.entry(a.clone()).or_default().push(v.clone()),
                Item::Call(id) => {
                    if !self.inert[*id as usize] {
                        self.flatten(&self.record(*id).items, effects, stores);
                    }
                }
                Item::Wrote(_) => {}
            }
        }
    }

    /// Put a finished record in the arena, or find its equal.
    /// `reads` is the hash of the record's reads in order, fed as they
    /// were noted (each location's hash and its version).
    pub(crate) fn intern(&mut self, mut rec: Record<M>, reads: u128) -> RecId {
        let mut h = Stable::new();
        h.word128(rec.name.0);
        h.word128(rec.result.version().0);
        h.word(rec.reads.len() as u64);
        h.word128(reads);
        h.word(rec.writes.len() as u64);
        for (a, v) in &rec.writes {
            h.word(hash64(a));
            h.word128(version_opt(v.as_ref()).0);
        }
        h.word(rec.items.len() as u64);
        for it in &rec.items {
            match it {
                Item::Out(e) => h.word128(Version::of(e).0),
                Item::Open(a) => {
                    h.word(1);
                    h.word(hash64(a));
                }
                Item::Store(a, v) => {
                    h.word(2);
                    h.word(hash64(a));
                    h.word128(v.version().0);
                }
                Item::Call(c) => {
                    h.word(3);
                    h.word128(self.record(*c).content.0);
                }
                Item::Wrote(a) => {
                    h.word(4);
                    h.word(hash64(a));
                }
            }
        }
        rec.content = Version(h.finish128());
        if let Some(&id) = self.dedup.get(&rec.content) {
            return id;
        }
        let (name, content) = (rec.name, rec.content);
        let inert = rec.items.iter().all(|it| match it {
            Item::Call(c) => self.inert[*c as usize],
            Item::Wrote(_) => true,
            _ => false,
        });
        let id = if let Some(id) = self.free.pop() {
            self.recs[id as usize] = Some(rec);
            id
        } else {
            self.recs.push(Some(rec));
            RecId::try_from(self.recs.len() - 1).expect("fewer than 2^32 records")
        };
        self.live += 1;
        self.writes_made += self.record(id).writes.len();
        if self.inert.len() <= id as usize {
            self.inert.resize(id as usize + 1, false);
        }
        self.inert[id as usize] = inert;
        self.memo_add(name, id);
        self.dedup.insert(content, id);
        id
    }

    fn memo_add(&mut self, name: Version, id: RecId) {
        Self::memo_add_to(&mut self.memo, name, id);
    }

    fn memo_add_to(memo: &mut Table<Version, Cands>, name: Version, id: RecId) {
        if let Some(c) = memo.get_mut(&name) {
            c.push(id);
        } else {
            memo.insert(name, Cands::one(id));
        }
    }

    /// Between two steps of a trip that removed many steps (a cascade
    /// gone cold retires the old steps after it): drop the records only
    /// they held ([`Runtime::collect`]), which the trip's new steps would
    /// otherwise replace at its end, both held at once.
    pub fn collect_retired(&mut self) {
        self.fold.release_removed_records();
        self.collect();
    }

    /// Drop the records no kept build reaches. A kept build's lean
    /// records (a step's, never looked up) are kept only if a live step
    /// of the fold holds them: an older run's, or a removed step's, goes,
    /// and its children, which a later call may hit, stay. Kept, a trip
    /// that ran most steps again kept the records of the trips before it,
    /// each holding its steps' writes: a thesis's settling grew by 1 GB.
    pub fn collect(&mut self) {
        let n = self.recs.len();
        let mut mark = alloc::vec![false; n];
        let mut seen = alloc::vec![false; n];
        // (each with whether it is held whatever it is: the fold's steps
        // are the state of every build to come)
        let mut stack: Vec<(RecId, bool)> =
            self.roots.iter().flatten().map(|&id| (id, false)).collect();
        stack.extend(self.fold.roots().map(|id| (id, true)));
        // (and, inside a trip, the calls its root made so far, which its
        // end walks, and the open step's)
        if let Some(root) = self.open.frames.first() {
            stack.extend(root.items.iter().filter_map(|it| match it {
                Item::Call(c) => Some((*c, true)),
                _ => None,
            }));
        }
        stack.extend(self.open.step_recs.iter().map(|&id| (id, true)));
        while let Some((id, held)) = stack.pop() {
            let i = id as usize;
            if held || !self.lean.get(i).copied().unwrap_or(false) {
                if core::mem::replace(&mut mark[i], true) {
                    continue;
                }
                seen[i] = true;
                stack.extend(self.record(id).children().map(|c| (c, true)));
            } else if !core::mem::replace(&mut seen[i], true) {
                stack.extend(self.record(id).children().map(|c| (c, false)));
            }
        }
        self.memo.clear();
        self.dedup.clear();
        self.live = 0;
        let mut writes = 0;
        for (i, r) in self.recs.iter_mut().enumerate() {
            let id = RecId::try_from(i).expect("fewer than 2^32 records");
            if r.is_some() && !mark[i] {
                *r = None;
                self.free.push(id);
                self.stats.collected += 1;
            }
            if let Some(r) = r {
                self.live += 1;
                writes += r.writes.len();
                let name = r.name;
                Self::memo_add_to(&mut self.memo, name, id);
                self.dedup.insert(r.content, id);
            }
        }
        // (a kept build's roots name only records kept: an id let go may
        // name another record later)
        for r in &mut self.roots {
            r.retain(|&id| mark[id as usize]);
        }
        self.live_after_gc = self.live;
        self.writes_after_gc = writes;
        self.writes_made = 0;
    }
}

#[derive(Clone)]
pub(crate) struct ReadE<A> {
    pub(crate) loc: Loc<A>,
    pub(crate) ver: Version,
}

pub(crate) struct Frame<M: Machine> {
    pub(crate) func: Option<M::Func>,
    pub(crate) name: Version,
    pub(crate) args: Vec<Version>,
    pub(crate) start: u64,
    pub(crate) reads: Vec<ReadE<M::Addr>>,
    /// The hash of `reads` so far, fed as each is pushed.
    pub(crate) rh: Stable,
    pub(crate) writes: Vec<(M::Addr, u64)>,
    pub(crate) items: Vec<Item<M>>,
    pub(crate) cost: u64,
    pub(crate) own: u64,
    /// A probed hit's body, run again (`open.rs`).
    pub(crate) quiet: bool,
    /// Whether the frame keeps its own reads and its body's first writes
    /// among its children ([`Item::Wrote`]): what a lookup verifies. A
    /// frame whose record is never looked up (a step outside check mode,
    /// `Runtime::begin_lean`) and the root keep neither; a read in them
    /// is noted only as the open step's (DESIGN 4.3 item 2).
    pub(crate) keep: bool,
}

impl<M: Machine> Frame<M> {
    pub(crate) fn root() -> Self {
        Frame {
            func: None,
            name: Version::ABSENT,
            args: Vec::new(),
            start: 0,
            reads: Vec::new(),
            rh: Stable::new(),
            writes: Vec::new(),
            items: Vec::new(),
            cost: 0,
            own: 0,
            quiet: false,
            keep: false,
        }
    }
}

/// The recording context of a trip: [`Cx`] over the open evaluator
/// (`open.rs`) and a [`MapStore`]. One evaluator serves the stub and
/// the engine; this is only the language's side of it.
struct Eval<'a, M: Machine> {
    m: &'a M,
    rt: &'a mut Runtime<M>,
    st: &'a mut MapStore<M>,
}

impl<M: Machine> Cx<M> for Eval<'_, M> {
    fn read(&mut self, a: &M::Addr) -> Option<M::Val> {
        let v = self.st.state.get(a).cloned();
        if self.rt.recording_open() {
            self.rt
                .note_read(&Loc::State(a.clone()), version_opt(v.as_ref()));
        }
        v
    }

    fn read_field(&mut self, a: &M::Addr, field: u32) -> Option<M::Val> {
        let v = self.st.state.get(a).and_then(|x| x.field(field));
        if self.rt.recording_open() {
            self.rt
                .note_read(&Loc::Field(a.clone(), field), version_opt(v.as_ref()));
        }
        v
    }

    fn write(&mut self, a: &M::Addr, v: Option<M::Val>) {
        self.st.set(a, v);
        self.rt.note_write(a);
    }

    fn effect(&mut self, e: M::Effect) {
        self.rt.note_effect(e);
    }

    fn call(&mut self, f: M::Func, args: &[M::Val]) -> M::Val {
        let m = self.m;
        self.rt.call(self.st, f, args, |st, rt| {
            m.run(f, args, &mut Eval { m, rt, st })
        })
    }

    fn open(&mut self, stream: &M::Addr) {
        self.rt.note_open(stream);
    }

    fn store(&mut self, stream: &M::Addr, line: M::Val) {
        self.rt.note_store(stream, line);
    }

    fn load(&mut self, stream: &M::Addr) -> Option<Stream<M::Val>> {
        let v = self.st.phi.get(stream).cloned();
        self.rt.note_loaded(stream);
        if self.rt.recording_open() {
            self.rt
                .note_read(&Loc::Phi(stream.clone()), stream_version(v.as_ref()));
        }
        v
    }

    fn cost(&mut self, units: u64) {
        self.rt.note_cost(units);
    }
}

/// The plain context: every call runs, nothing is recorded.
struct Plain<'a, M: Machine> {
    m: &'a M,
    state: PMap<M::Addr, M::Val>,
    phi: &'a PMap<M::Addr, Stream<M::Val>>,
    effects: Vec<M::Effect>,
    stores: BTreeMap<M::Addr, Vec<M::Val>>,
    loaded: Vec<M::Addr>,
    calls: u64,
    on_effect: Option<fn(EffectKind)>,
}

impl<M: Machine> Cx<M> for Plain<'_, M> {
    fn read(&mut self, a: &M::Addr) -> Option<M::Val> {
        self.state.get(a).cloned()
    }
    fn read_field(&mut self, a: &M::Addr, field: u32) -> Option<M::Val> {
        self.state.get(a).and_then(|x| x.field(field))
    }
    fn write(&mut self, a: &M::Addr, v: Option<M::Val>) {
        match v {
            Some(v) => {
                self.state.insert(a.clone(), v);
            }
            None => {
                self.state.remove(a);
            }
        }
    }
    fn effect(&mut self, e: M::Effect) {
        if let Some(h) = self.on_effect {
            h(EffectKind::Output);
        }
        self.effects.push(e);
    }
    fn call(&mut self, f: M::Func, args: &[M::Val]) -> M::Val {
        self.calls += 1;
        let m = self.m;
        m.run(f, args, self)
    }
    fn open(&mut self, stream: &M::Addr) {
        if let Some(h) = self.on_effect {
            h(EffectKind::Store);
        }
        self.stores.entry(stream.clone()).or_default();
    }
    fn store(&mut self, stream: &M::Addr, line: M::Val) {
        if let Some(h) = self.on_effect {
            h(EffectKind::Store);
        }
        self.stores.entry(stream.clone()).or_default().push(line);
    }
    fn load(&mut self, stream: &M::Addr) -> Option<Stream<M::Val>> {
        if !self.loaded.contains(stream) {
            self.loaded.push(stream.clone());
        }
        self.phi.get(stream).cloned()
    }
    fn cost(&mut self, _units: u64) {}
}
