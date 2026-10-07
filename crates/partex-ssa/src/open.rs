//! The runtime opened to a language that owns its state (`DESIGN.md`
//! §7.17.10 step 2): the runtime is the memo and the recorder, the
//! language keeps its state where it is, behind a [`Store`].
//!
//! The engine keeps its state in its own arrays and will not pass a
//! context around, so the recorder is driven from outside: its tracker
//! reports each read ([`Runtime::note_read`], a location and the version
//! the store gives it) and each write ([`Runtime::note_write`]), and a
//! call is either a closure ([`Runtime::call`], re-entrant: the body gets
//! the store and the runtime back, so it can make nested calls) or a
//! [`Runtime::begin`]/[`Runtime::end`] pair, for a call that spans steps
//! of the engine's main loop (a paragraph, from one clean point to the
//! next). The records, the memo by name, the verification in order and
//! the text form are [`Runtime`]'s; only the state moved out.
//!
//! A write is backdated by the store: an equal value leaves the version.
//! The record keeps the write anyway (7.17.2's second rule), with the
//! value the store holds at the call's end.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::fold::StepId;
use crate::hash::{Version, hash64};
use crate::machine::{Loc, Machine, Stream, name_of};
use crate::pmap::PMap;
use crate::pvec::PVec;
use crate::runtime::{Frame, Item, ReadE, RecId, Record, Runtime, Status, TripLog};
use crate::table::{ByHash, Table};
use crate::value::{Value, version_opt};

/// A language's state as the runtime sees it: the version of a
/// location, and the values at its addresses.
pub trait Store<M: Machine + ?Sized> {
    /// The version now at `loc` (a slot, a field of one, or a stream's φ).
    fn version(&self, loc: &Loc<M::Addr>) -> Version;
    fn get(&self, a: &M::Addr) -> Option<M::Val>;
    /// Put `v` at `a` (`None`: remove it). An equal value must leave the
    /// version as it is (backdating).
    fn set(&mut self, a: &M::Addr, v: Option<M::Val>);
}

/// The persistent map as a store: the stub's state, and the φ.
pub struct MapStore<M: Machine> {
    pub state: PMap<M::Addr, M::Val>,
    pub phi: PMap<M::Addr, Stream<M::Val>>,
}

impl<M: Machine> Default for MapStore<M> {
    fn default() -> Self {
        MapStore {
            state: PMap::new(),
            phi: PMap::new(),
        }
    }
}

impl<M: Machine> Store<M> for MapStore<M> {
    fn version(&self, loc: &Loc<M::Addr>) -> Version {
        match loc {
            Loc::State(a) => version_opt(self.state.get(a)),
            Loc::Field(a, f) => version_opt(self.state.get(a).and_then(|v| v.field(*f)).as_ref()),
            Loc::Phi(a) => self.phi.get(a).map_or(Version::ABSENT, PVec::version),
        }
    }

    fn get(&self, a: &M::Addr) -> Option<M::Val> {
        self.state.get(a).cloned()
    }

    fn set(&mut self, a: &M::Addr, v: Option<M::Val>) {
        if version_opt(self.state.get(a)) == version_opt(v.as_ref()) {
            return;
        }
        match v {
            Some(v) => {
                self.state.insert(a.clone(), v);
            }
            None => {
                self.state.remove(a);
            }
        }
    }
}

/// A slot's serials: its last write, its last recorded read, the start
/// of the frame that recorded it (a frame records a slot once after each
/// write; its children record their own), and the start of the step that
/// noted it as read from outside the step, and the step (its serial) a
/// group's end gave the slot back the value it began with, not written
/// since ([`Runtime::unwrite`]): to the step, its entry value again.
#[derive(Clone, Copy, Default)]
struct Stamp {
    w: u64,
    r: u64,
    f: u64,
    s: u64,
    u: u64,
}

/// A location's last recorded read, the frame that recorded it, and the
/// step that noted it (non-dense slots: [`Stamp`]'s last three).
#[derive(Clone, Copy, Default)]
struct RStamp {
    r: u64,
    f: u64,
    s: u64,
}

/// Where the entry of a slot's last write is: the frame's depth and the
/// index in its writes, so a rewrite finds it without a search. (Apart
/// from the stamp, so a read touches 16 bytes per slot.)
#[derive(Clone, Copy, Default)]
struct Pos {
    at: u32,
    ix: u32,
}

/// Slot `i` of family `f` in a dense array, grown as needed.
#[inline]
fn dense_at<T: Copy + Default>(v: &mut Vec<Vec<T>>, f: usize, i: usize) -> &mut T {
    if f >= v.len() {
        v.resize_with(f + 1, Vec::new);
    }
    let fam = &mut v[f];
    if i >= fam.len() {
        fam.resize((i + 1).next_power_of_two().max(64), T::default());
    }
    &mut fam[i]
}

/// A dense slot's location hash.
#[inline]
fn dense_hash(f: usize, i: usize) -> u64 {
    ((f as u64) << 40 ^ i as u64 ^ 0x6465_6e73_6500_0000)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .rotate_left(31)
}

/// The read-serial table's hash of a location, from its address's.
#[inline]
fn loc_hash<A>(loc: &Loc<A>, ha: u64) -> u64 {
    let t = match loc {
        Loc::State(_) => 1,
        Loc::Field(_, i) => 2 + (u64::from(*i) << 2),
        Loc::Phi(_) => 3,
    };
    (ha ^ t.wrapping_mul(0x9e37_79b9_7f4a_7c15)).rotate_left(29)
}

/// When a step's run made its reads from outside it and last wrote each
/// address, on the engine's clock (its commands), kept only with timing
/// on ([`Runtime::set_timing`]): a measurement of how far into a step
/// each read waits for its definition and each definition is made, for
/// the parallelism a dependency graph allows (`PARTEX_SSA_DAG`).
#[derive(Clone, Debug, Default)]
pub struct StepTimes<A> {
    /// The clock when the run began.
    pub began: u64,
    /// When each outside read was made, in the order of the step's reads.
    pub reads: Vec<u64>,
    /// When each address written was last written.
    pub wrote: Vec<(A, u64)>,
}

/// What a lookup found.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Found<A> {
    /// A record whose reads all verified.
    Hit(RecId),
    /// No record by that name.
    New,
    /// Records by that name, none verified; the first read that differed.
    Miss(Loc<A>),
}

/// The recorder's state for one trip of an open build.
pub(crate) struct Open<M: Machine> {
    wser: Table<ByHash<M::Addr>, (Stamp, Pos)>,
    /// A location's last recorded read, the frame and the step.
    rser: Table<ByHash<Loc<M::Addr>>, RStamp>,
    /// The stamps of dense slots ([`Machine::dense`]), by family, for
    /// reads of the slot itself: no hashing on the engine's hot path.
    dense: Vec<Vec<Stamp>>,
    /// The dense slots' [`Pos`]es.
    dpos: Vec<Vec<Pos>>,
    serial: u64,
    pub(crate) frames: Vec<Frame<M>>,
    pub(crate) statuses: Vec<Status<M::Addr>>,
    /// The streams the trip loaded (a load, or a φ read a hit verified).
    pub(crate) loaded: Vec<M::Addr>,
    /// The calls whose bodies ran, in program order.
    pub(crate) reran: Vec<(M::Func, Version)>,
    /// Frames re-run under a hit (check mode): their children's statuses
    /// are not the trace's (a hit's children are reused unseen).
    quiet: usize,
    trip: usize,
    /// The fold's step open now (7.17.3): its id and the serial it began
    /// at, the addresses read from outside it (with their hashes), and
    /// the top level's calls it made.
    pub(crate) step: Option<(StepId, u64)>,
    /// A soft read in progress ([`Runtime::note_read_soft`]): a read from
    /// outside the step is not the step's (yet), and `soft_new` says one
    /// was made.
    soft: bool,
    soft_new: bool,
    /// (debug) writes made with no call open while a step was.
    pub(crate) root_writes: u64,
    /// (debug) writes made while no step was open.
    pub(crate) stepless_writes: u64,
    pub(crate) step_reads: Vec<(u64, M::Addr)>,
    /// With versions kept ([`Runtime::keep_step_versions`], a worker's
    /// run, DESIGN 3.10, "Commit"): the version each read of
    /// `step_reads` found, in the same order; the version of the read
    /// being noted; and each soft read's version, for the read the
    /// step's end may make of it ([`Open::force_step_read`]).
    pub(crate) step_vers: Option<Vec<Version>>,
    cur_ver: Version,
    soft_vers: Table<ByHash<M::Addr>, Version>,
    pub(crate) step_recs: Vec<RecId>,
    /// Timing ([`StepTimes`], a measurement, off unless asked for): the
    /// engine's clock, when the open step began, when it made each read
    /// in `step_reads`, and when it last wrote each address.
    pub(crate) timing: bool,
    pub(crate) clock: u64,
    step_began: u64,
    pub(crate) step_read_at: Vec<u64>,
    pub(crate) step_wrote_at: Table<ByHash<M::Addr>, u64>,
}

impl<M: Machine> Open<M> {
    pub(crate) fn new() -> Self {
        Open {
            wser: Table::new(),
            rser: Table::new(),
            dense: Vec::new(),
            dpos: Vec::new(),
            serial: 0,
            frames: alloc::vec![Frame::root()],
            statuses: Vec::new(),
            loaded: Vec::new(),
            reran: Vec::new(),
            quiet: 0,
            trip: 0,
            step: None,
            soft: false,
            soft_new: false,
            root_writes: 0,
            stepless_writes: 0,
            step_reads: Vec::new(),
            step_vers: None,
            cur_ver: Version::ABSENT,
            soft_vers: Table::new(),
            step_recs: Vec::new(),
            timing: false,
            clock: 0,
            step_began: 0,
            step_read_at: Vec::new(),
            step_wrote_at: Table::new(),
        }
    }

    /// A fresh trip's state in place of this one's, the timing switch
    /// and the clock kept.
    ///
    /// The dense slots' tables are kept as they are, and the serial goes
    /// on: every stamp in them is older than the new trip's frames and
    /// steps, as a new table's zeros are, so no test tells them apart (a
    /// write's entry is live only if its frame, all of the new trip,
    /// holds that serial). Made anew, they were grown and zeroed again at
    /// each trip as far as the highest slot the trip touched (the eqtb's
    /// entries of the names in the hash's extra room, the pool's newest
    /// strings): a one-page document's keystroke spent most of its time
    /// noting writes doing that.
    fn renew(&mut self) {
        let dense = core::mem::take(&mut self.dense);
        let dpos = core::mem::take(&mut self.dpos);
        let (timing, clock, serial) = (self.timing, self.clock, self.serial);
        let versions = self.step_vers.is_some();
        *self = Open::new();
        self.dense = dense;
        self.dpos = dpos;
        self.timing = timing;
        self.clock = clock;
        self.serial = serial;
        if versions {
            self.step_vers = Some(Vec::new());
        }
    }

    /// A fresh trip ([`Open::renew`]), its vectors sized like the last
    /// one's.
    pub(crate) fn reset(&mut self, statuses: Vec<Status<M::Addr>>, reran: usize) {
        self.renew();
        self.statuses = statuses;
        self.reran = Vec::with_capacity(reran);
    }

    /// The open step's read of `a` from outside it, noted (with its time,
    /// if timing).
    #[inline]
    fn push_step_read(&mut self, ha: u64, a: &M::Addr) {
        self.step_reads.push((ha, a.clone()));
        if let Some(v) = &mut self.step_vers {
            v.push(self.cur_ver);
        }
        if self.timing {
            self.step_read_at.push(self.clock);
        }
    }

    fn last_write(&self, a: &M::Addr) -> u64 {
        if let Some((f, i)) = M::dense(a) {
            return self.dense.get(f).and_then(|v| v.get(i)).map_or(0, |s| s.w);
        }
        self.wser
            .get_by(hash64(a), |k| k.0 == *a)
            .map_or(0, |s| s.0.w)
    }

    /// A read of `loc` by the running call, its version made only if the
    /// read is recorded: a read of a slot the call wrote before, itself or
    /// through a child, or read before since its last write, returns at
    /// once (for a dense slot, one array access).
    ///
    /// A read is the innermost frame's own (7.17.2) iff the slot's last
    /// write `w` is older than the frame; it is recorded unless the frame
    /// recorded it already since `w` (the last recorded read `r` by the
    /// frame that began at `f`). A child's reads are its own, in its
    /// record, and never its parent's.
    #[inline]
    pub(crate) fn note_read_with(&mut self, loc: &Loc<M::Addr>, ver: impl FnOnce() -> Version) {
        if self.step_vers.is_some() {
            // (a worker's run: each read's version made, for the reads of
            // its step from outside it, which the commit compares)
            let v = ver();
            self.cur_ver = v;
            return self.note_read_inner(loc, || v);
        }
        self.note_read_inner(loc, ver);
    }

    #[inline]
    fn note_read_inner(&mut self, loc: &Loc<M::Addr>, ver: impl FnOnce() -> Version) {
        let Some(top) = self.frames.last() else {
            return;
        };
        if !top.keep {
            // (a frame that keeps no reads, or the root: the read is only
            // the open step's, if from outside it; no version is made)
            self.note_step_read(loc);
            return;
        }
        let start = top.start;
        let step = self.step.map_or(0, |s| s.1);
        let dense = match loc {
            Loc::State(a) => M::dense(a),
            _ => None,
        };
        let lh = if let Some((f, i)) = dense {
            let st = dense_at(&mut self.dense, f, i);
            if st.w >= start || (!self.soft && st.f == start && st.r > st.w) {
                // (the call has the value already; the step, if a group's end
                // gave the slot back what it began with, reads it from
                // outside)
                if step != 0 && st.u == step && st.s != step {
                    if self.soft {
                        self.soft_new = true;
                    } else {
                        st.s = step;
                        let a = loc.addr();
                        self.push_step_read(hash64(a), a);
                    }
                }
                return;
            }
            // (a soft read leaves the stamps: a read after it is recorded)
            if !self.soft {
                self.serial += 1;
                st.f = start;
                st.r = self.serial;
            }
            // (a read from outside the step: its readers, 7.17.3; or of the
            // value the step began with, a group's end gave back)
            if step != 0 && (st.w < step || st.u == step) && st.s != step {
                if self.soft {
                    self.soft_new = true;
                } else {
                    st.s = step;
                    let a = loc.addr();
                    self.push_step_read(hash64(a), a);
                }
            }
            dense_hash(f, i)
        } else {
            let a = loc.addr();
            let ha = hash64(a);
            let w = match loc {
                Loc::Phi(_) => 0,
                _ if M::dense(a).is_some() => self.last_write(a),
                _ => self.wser.get_by(ha, |k| k.0 == *a).map_or(0, |s| s.0.w),
            };
            if w >= start {
                return;
            }
            let slot = self.rser.entry_by(
                loc_hash(loc, ha),
                |k| k.0 == *loc,
                || ByHash(loc.clone()),
                RStamp::default(),
            );
            if !self.soft && slot.f == start && slot.r > w {
                return;
            }
            if !self.soft {
                self.serial += 1;
                slot.r = self.serial;
                slot.f = start;
            }
            if step != 0 && w < step && slot.s != step {
                if self.soft {
                    self.soft_new = true;
                } else {
                    slot.s = step;
                    self.push_step_read(ha, a);
                }
            }
            loc_hash(loc, ha)
        };
        let ver = ver();
        let top = self.frames.last_mut().expect("a frame");
        top.rh.word(lh);
        top.rh.word128(ver.0);
        top.reads.push(ReadE {
            loc: loc.clone(),
            ver,
        });
    }

    /// A read of `loc` that is only the open step's: from outside a hit
    /// put in place in the step (7.17.3, "Hits applied inside a step that
    /// runs again", item 3), or in a frame that keeps no reads. It is a
    /// read of the step unless the step wrote the slot before, noted once
    /// per step, as [`Open::note_read_with`] notes the body's; the
    /// frame's reads are not touched (a hit's stay in its record).
    #[inline]
    fn note_step_read(&mut self, loc: &Loc<M::Addr>) {
        let step = self.step.map_or(0, |s| s.1);
        if step == 0 {
            return;
        }
        let a = loc.addr();
        if let (Loc::State(_), Some((f, i))) = (loc, M::dense(a)) {
            let st = dense_at(&mut self.dense, f, i);
            if (st.w < step || st.u == step) && st.s != step {
                if self.soft {
                    self.soft_new = true;
                } else {
                    st.s = step;
                    self.push_step_read(hash64(a), a);
                }
            }
            return;
        }
        let ha = hash64(a);
        let w = match loc {
            Loc::Phi(_) => 0,
            _ if M::dense(a).is_some() => self.last_write(a),
            _ => self.wser.get_by(ha, |k| k.0 == *a).map_or(0, |s| s.0.w),
        };
        if w >= step {
            return;
        }
        let slot = self.rser.entry_by(
            loc_hash(loc, ha),
            |k| k.0 == *loc,
            || ByHash(loc.clone()),
            RStamp::default(),
        );
        if slot.s != step {
            if self.soft {
                self.soft_new = true;
            } else {
                slot.s = step;
                self.push_step_read(ha, a);
            }
        }
    }

    /// Whether the open step read dense slot `a` from outside it (its
    /// stamp; every read of a dense slot the step notes sets it), or
    /// `None` for a slot that is not dense.
    fn dense_step_read(&self, a: &M::Addr) -> Option<bool> {
        let (f, i) = M::dense(a)?;
        let step = self.step.map_or(0, |s| s.1);
        Some(
            step != 0
                && self
                    .dense
                    .get(f)
                    .and_then(|v| v.get(i))
                    .is_some_and(|st| st.s == step),
        )
    }

    /// A step begins or its run is dropped: no versions of its reads yet.
    fn clear_step_versions(&mut self) {
        if let Some(v) = &mut self.step_vers {
            v.clear();
            if !self.soft_vers.is_empty() {
                self.soft_vers = Table::new();
            }
        }
    }

    /// `a` read by the open step from outside it, made at a soft read
    /// before the step wrote it ([`Runtime::end_step_soft`]): recorded
    /// once, whatever the step wrote since.
    pub(crate) fn force_step_read(&mut self, a: &M::Addr) {
        // (the caller passes only slots not read yet)
        let step = self.step.map_or(0, |s| s.1);
        if step == 0 {
            return;
        }
        if let Some((f, i)) = M::dense(a) {
            dense_at(&mut self.dense, f, i).s = step;
        }
        let ha = hash64(a);
        if self.step_vers.is_some() {
            // (the version the soft read found: the step's entry value)
            self.cur_ver = self
                .soft_vers
                .get_by(ha, |k| k.0 == *a)
                .copied()
                .unwrap_or(Version::ABSENT);
        }
        self.push_step_read(ha, a);
    }

    /// The running call wrote `a`.
    ///
    /// A frame keeps one entry per slot, with the serial of the slot's
    /// last write: a rewrite in the frame updates its entry in place, and
    /// a write in a call nested in the frame that holds the entry makes
    /// that entry dead (serial 0) and adds one to the call's frame, which
    /// its end hands to the parent. At a call's end its live entries in
    /// serial order are its net writes, in the order of their last writes.
    #[inline]
    pub(crate) fn note_write(&mut self, a: &M::Addr) {
        self.note_write_by(a, true);
    }

    /// [`Open::note_write`] by the frame's own body (`own`), or of a child
    /// put in place by a hit, whose writes are its parent's as a closed
    /// child's are ([`Open::adopt`]), with no [`Item::Wrote`].
    #[inline]
    fn note_write_by(&mut self, a: &M::Addr, own: bool) {
        let d = self.frames.len() - 1;
        if self.step.is_none() {
            self.stepless_writes += 1;
        }
        if d == 0 {
            if self.step.is_some() {
                self.root_writes += 1;
            }
            return;
        }
        if self.timing && self.step.is_some() {
            let now = self.clock;
            *self
                .step_wrote_at
                .entry_by(hash64(a), |k| k.0 == *a, || ByHash(a.clone()), 0) = now;
        }
        self.serial += 1;
        let s = self.serial;
        let (st, pos) = if let Some((f, i)) = M::dense(a) {
            let d = dense_at(&mut self.dense, f, i);
            d.u = 0;
            (&mut d.w, dense_at(&mut self.dpos, f, i))
        } else {
            let e = self.wser.entry_by(
                hash64(a),
                |k| k.0 == *a,
                || ByHash(a.clone()),
                Default::default(),
            );
            (&mut e.0.w, &mut e.1)
        };
        let old = *st;
        *st = s;
        let (at, ix) = (pos.at as usize, pos.ix as usize);
        let live = old != 0
            && self
                .frames
                .get(at)
                .and_then(|f| f.writes.get(ix))
                .is_some_and(|e| e.1 == old);
        if live {
            if at == d {
                self.frames[d].writes[ix].1 = s;
                return;
            }
            self.frames[at].writes[ix].1 = 0;
        }
        let top = &mut self.frames[d];
        pos.at = u32::try_from(d).expect("a depth below 2^32");
        pos.ix = u32::try_from(top.writes.len()).expect("fewer than 2^32 writes");
        top.writes.push((a.clone(), s));
        // (the body's first write of the slot, among its children: a later
        // child's read of it is inside the call, for a lookup's walk)
        if own && top.keep {
            top.items.push(Item::Wrote(a.clone()));
        }
    }

    /// The live entries of a closed frame's writes, in serial order.
    fn net(writes: Vec<(M::Addr, u64)>) -> Vec<(M::Addr, u64)> {
        let mut net: Vec<_> = writes.into_iter().filter(|e| e.1 != 0).collect();
        net.sort_unstable_by_key(|e| e.1);
        net
    }

    /// A closed call's net writes, handed to its parent (the innermost
    /// frame now): their stamps point at their new places.
    fn adopt(&mut self, net: &[(M::Addr, u64)]) {
        let d = self.frames.len() - 1;
        let at = u32::try_from(d).expect("a depth below 2^32");
        let parent = self.frames.last_mut().expect("a parent frame");
        for (a, s) in net {
            let ix = u32::try_from(parent.writes.len()).expect("fewer than 2^32 writes");
            let pos = if let Some((f, i)) = M::dense(a) {
                dense_at(&mut self.dpos, f, i)
            } else {
                &mut self
                    .wser
                    .entry_by(
                        hash64(a),
                        |k| k.0 == *a,
                        || ByHash(a.clone()),
                        Default::default(),
                    )
                    .1
            };
            *pos = Pos { at, ix };
            parent.writes.push((a.clone(), *s));
        }
    }
}

impl<M: Machine> Runtime<M> {
    /// Start trip `trip` of an open build: a fresh root, no reads or
    /// writes; trip 0 starts a build.
    pub fn open_trip(&mut self, trip: usize) {
        self.open.renew();
        self.open.trip = trip;
        if trip == 0 {
            self.stats.builds += 1;
            self.clear_log();
            self.start_phi = self.streams().clone();
        }
    }

    /// End the trip: its root's items go to the trace and are kept as a
    /// build's roots; the next build's lookups find them.
    pub fn close_trip(&mut self) {
        while self.open.frames.len() > 1 {
            // a call left open (the job ended inside it) ends here
            self.end_absent();
        }
        let root = core::mem::replace(&mut self.open.frames[0], Frame::root()).items;
        let statuses = core::mem::take(&mut self.open.statuses);
        let mut ids = Vec::new();
        let mut streams: BTreeMap<M::Addr, usize> = BTreeMap::new();
        let mut effects = Vec::new();
        let mut stores: BTreeMap<M::Addr, Vec<M::Val>> = BTreeMap::new();
        for it in &root {
            if let Item::Call(id) = it {
                ids.push(*id);
            }
        }
        self.flatten(&root, &mut effects, &mut stores);
        for (k, v) in &stores {
            streams.insert(k.clone(), v.len());
        }
        self.stats.trips += 1;
        self.log.push(TripLog {
            index: self.open.trip,
            phi: PMap::new(),
            root,
            statuses,
            changed: Vec::new(),
            streams,
        });
        self.keep_roots(ids);
    }

    /// The depth of the open calls (0: the root).
    #[must_use]
    pub fn depth(&self) -> usize {
        self.open.frames.len() - 1
    }

    pub(crate) fn recording_open(&self) -> bool {
        self.open.frames.len() > 1
    }

    /// Find a record named `f(args)` whose reads all verify in `store`.
    pub fn lookup<S: Store<M> + ?Sized>(
        &mut self,
        store: &S,
        f: M::Func,
        args: &[Version],
    ) -> Found<M::Addr> {
        self.lookup_named(store, name_of(f, args))
    }

    fn lookup_named<S: Store<M> + ?Sized>(&mut self, store: &S, name: Version) -> Found<M::Addr> {
        let Some(ids) = self.memo.get(&name) else {
            return Found::New;
        };
        let mut first = None;
        let mut verified = 0;
        let mut found = None;
        let mut inside = Table::new();
        for id in ids.iter() {
            inside.clear();
            match self.walk(store, id, &mut inside, &mut verified) {
                Ok(()) => {
                    found = Some(id);
                    break;
                }
                Err(l) => {
                    if first.is_none() {
                        first = Some(l);
                    }
                }
            }
        }
        self.stats.reads_verified += verified;
        match (found, first) {
            (Some(id), _) => Found::Hit(id),
            (None, Some(l)) => Found::Miss(l),
            (None, None) => Found::New,
        }
    }

    /// Whether record `id` is a hit in `store` (7.17.2): the reads of its
    /// subtree from outside it hold the versions now current, or the
    /// first that does not.
    pub fn verify<S: Store<M> + ?Sized>(
        &mut self,
        store: &S,
        id: RecId,
    ) -> Result<(), Loc<M::Addr>> {
        let mut verified = 0;
        let r = self.walk(store, id, &mut Table::new(), &mut verified);
        self.stats.reads_verified += verified;
        r
    }

    /// [`Runtime::verify`]'s walk over `id`'s subtree in program order.
    /// `inside` holds the slots written inside the call being verified
    /// before `id` began: the call's body's first writes as they pass
    /// ([`Item::Wrote`]) and each child's writes once it ends. A record's
    /// own reads are all from outside it, so a read is from outside the
    /// verified call iff its slot is not in `inside`.
    fn walk<S: Store<M> + ?Sized>(
        &self,
        store: &S,
        id: RecId,
        inside: &mut Table<ByHash<M::Addr>, ()>,
        verified: &mut u64,
    ) -> Result<(), Loc<M::Addr>> {
        let rec = self.record(id);
        for (loc, ver) in &rec.reads {
            let a = loc.addr();
            if inside.get_by(hash64(a), |k| k.0 == *a).is_some() {
                continue;
            }
            *verified += 1;
            if store.version(loc) != *ver {
                return Err(loc.clone());
            }
        }
        for it in &rec.items {
            match it {
                Item::Wrote(a) => {
                    inside.entry_by(hash64(a), |k| k.0 == *a, || ByHash(a.clone()), ());
                }
                Item::Call(c) => {
                    self.walk(store, *c, inside, verified)?;
                    for (a, _) in self.writes(*c) {
                        inside.entry_by(hash64(a), |k| k.0 == *a, || ByHash(a.clone()), ());
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Count a lookup's outcome and note it for the trace.
    fn account(&mut self, found: &Found<M::Addr>) {
        let status = match found {
            Found::Hit(_) => {
                self.stats.hits += 1;
                Status::Hit
            }
            Found::New => {
                self.stats.misses += 1;
                self.stats.fresh += 1;
                Status::New
            }
            Found::Miss(l) => {
                self.stats.misses += 1;
                Status::Miss(l.clone())
            }
        };
        if self.open.quiet == 0 {
            self.open.statuses.push(status);
        }
    }

    /// A hit applied: its writes' values are made current (one store per
    /// written address) and noted, and it is the running call's child,
    /// its reads staying in its record (7.17.2). Its body does not run.
    pub fn apply<S: Store<M> + ?Sized>(&mut self, store: &mut S, id: RecId) -> M::Val {
        let mut loaded = Vec::new();
        self.phi_reads(id, &mut loaded);
        for a in loaded {
            self.note_loaded(&a);
        }
        let Runtime { recs, wa, open, .. } = self;
        // (its reads from outside it are the open step's, before its
        // writes make their slots the step's own)
        if open.step.is_some() {
            outside_reads(recs, wa, id, &mut Table::new(), &mut |l| {
                open.note_step_read(l);
            });
        }
        let rec = recs[id as usize].as_ref().expect("a live record");
        for (a, v) in wa.get(rec.w) {
            store.set(a, v.clone());
            open.note_write_by(a, false);
        }
        let top = open.frames.last_mut().expect("a frame");
        top.items.push(Item::Call(id));
        top.cost += rec.cost;
        rec.result.clone()
    }

    /// The streams record `id`'s subtree loaded (its φ reads).
    fn phi_reads(&self, id: RecId, out: &mut Vec<M::Addr>) {
        let rec = self.record(id);
        for (loc, _) in &rec.reads {
            if let Loc::Phi(a) = loc
                && !out.contains(a)
            {
                out.push(a.clone());
            }
        }
        for c in rec.children() {
            self.phi_reads(c, out);
        }
    }

    /// Evaluate `f(args)`: a hit's writes are applied, a miss runs `body`
    /// with the store and the runtime, recording.
    pub fn call<S, B>(&mut self, store: &mut S, f: M::Func, args: &[M::Val], body: B) -> M::Val
    where
        S: Store<M> + ?Sized,
        B: FnOnce(&mut S, &mut Self) -> M::Val,
    {
        if !self.cfg.record {
            self.stats.plain_calls += 1;
            return body(store, self);
        }
        // (the arguments' versions on the stack: a hit allocates nothing)
        let mut small = [Version::ABSENT; 4];
        let heap: Vec<Version>;
        let argv: &[Version] = if args.len() <= small.len() {
            for (v, a) in small.iter_mut().zip(args) {
                *v = a.version();
            }
            &small[..args.len()]
        } else {
            heap = args.iter().map(Value::version).collect();
            &heap
        };
        let name = name_of(f, argv);
        let found = self.lookup_named(store, name);
        self.account(&found);
        if let Found::Hit(id) = found {
            return self.apply(store, id);
        }
        self.begin_named(f, name, argv.to_vec(), false, true);
        let result = body(store, self);
        self.end(store, result.clone());
        result
    }

    /// Evaluate `f(args)` for its status only: the caller runs the body
    /// either way, between [`Runtime::begin`] and [`Runtime::end`] (check
    /// mode, and a hit the language cannot apply). A hit's body
    /// is then run quietly: its children's statuses stay out of the trace.
    pub fn probe<S: Store<M> + ?Sized>(
        &mut self,
        store: &S,
        f: M::Func,
        args: &[Version],
    ) -> Found<M::Addr> {
        let found = self.lookup(store, f, args);
        self.account(&found);
        found
    }

    /// Open a call frame named `f(args)`; its body's reads and writes are
    /// noted until [`Runtime::end`].
    pub fn begin(&mut self, f: M::Func, args: Vec<Version>) {
        self.begin_quiet(f, args, false);
    }

    /// [`Runtime::begin`], with the children's statuses left out of the
    /// trace if `quiet` (the body of a probed hit).
    pub fn begin_quiet(&mut self, f: M::Func, args: Vec<Version>, quiet: bool) {
        let name = name_of(f, &args);
        self.begin_named(f, name, args, quiet, true);
    }

    /// [`Runtime::begin`] for a call whose record is never looked up (a
    /// step outside check mode, DESIGN 4.3 item 2): its frame keeps no
    /// reads of its own and no [`Item::Wrote`], only its net writes, its
    /// effects and its children. A read in it is noted as the open
    /// step's, if from outside the step (its readers in the fold), and no
    /// version is made for it.
    pub fn begin_lean(&mut self, f: M::Func, args: Vec<Version>) {
        let name = name_of(f, &args);
        self.begin_named(f, name, args, false, false);
    }

    fn begin_named(
        &mut self,
        f: M::Func,
        name: Version,
        args: Vec<Version>,
        quiet: bool,
        keep: bool,
    ) {
        self.open.serial += 1;
        if quiet {
            self.open.quiet += 1;
        } else if self.open.quiet == 0 {
            self.open.reran.push((f, name));
        }
        let items = self
            .sizes
            .items
            .get(&Version(u128::from(hash64(&f))))
            .map_or_else(Vec::new, |&n| Vec::with_capacity(n));
        self.open.frames.push(Frame {
            func: Some(f),
            name,
            args,
            start: self.open.serial,
            reads: Vec::new(),
            rh: crate::hash::Stable::new(),
            writes: Vec::new(),
            items,
            cost: 0,
            own: 0,
            quiet,
            keep,
        });
    }

    /// Close the innermost call with `result`: its record is interned
    /// (or found equal to one that is) and is its parent's child.
    pub fn end<S: Store<M> + ?Sized>(&mut self, store: &S, result: M::Val) -> RecId {
        let fr = self.open.frames.pop().expect("an open call");
        let net = Open::<M>::net(fr.writes);
        let writes = net.iter().map(|(a, _)| (a.clone(), store.get(a))).collect();
        self.close_frame(
            fr.func, fr.name, fr.args, fr.reads, &fr.rh, &net, writes, fr.items, fr.cost, fr.own,
            fr.quiet, fr.keep, result,
        )
    }

    /// Close the innermost call without a store (the job ended inside
    /// it): its writes keep no values.
    fn end_absent(&mut self) {
        let fr = self.open.frames.pop().expect("an open call");
        let net = Open::<M>::net(fr.writes);
        let writes = net.iter().map(|(a, _)| (a.clone(), None)).collect();
        let result = self.open_default_result(&fr.items);
        self.close_frame(
            fr.func, fr.name, fr.args, fr.reads, &fr.rh, &net, writes, fr.items, fr.cost, fr.own,
            fr.quiet, fr.keep, result,
        );
    }

    fn open_default_result(&self, _items: &[Item<M>]) -> M::Val {
        self.default_val
            .clone()
            .expect("Runtime::set_default_value before an open build")
    }

    #[allow(clippy::too_many_arguments)]
    fn close_frame(
        &mut self,
        func: Option<M::Func>,
        name: Version,
        args: Vec<Version>,
        reads: Vec<ReadE<M::Addr>>,
        rh: &crate::hash::Stable,
        net: &[(M::Addr, u64)],
        writes: Vec<(M::Addr, Option<M::Val>)>,
        mut items: Vec<Item<M>>,
        cost: u64,
        own: u64,
        quiet: bool,
        keep: bool,
        result: M::Val,
    ) -> RecId {
        if quiet {
            self.open.quiet -= 1;
        }
        // (the child's reads stay in its record, 7.17.2; its writes are
        // its parent's too, and so inside the parent)
        if self.open.frames.len() > 1 {
            self.open.adopt(net);
        }
        if items.len() >= 256 {
            let f = func.expect("a call frame");
            self.sizes
                .items
                .insert(Version(u128::from(hash64(&f))), items.len());
        }
        // (the record keeps its items for the session: the room the frame
        // began with, the last large frame's of its function, goes back.
        // Kept, every later call of a function that once had 256 items
        // held that room untouched: 16 KB a record, the course's 1.68 M
        // records 27 GB of address space at 13 GB resident)
        items.shrink_to_fit();
        let rec = Record {
            func: func.expect("a call frame"),
            name,
            args,
            result,
            reads: reads.into_iter().map(|e| (e.loc, e.ver)).collect(),
            w: crate::runtime::WSpan::default(),
            items,
            cost: cost + own,
            own,
            content: Version::ABSENT,
        };
        let total = rec.cost;
        self.stats.cost_rerun += own;
        let live = self.live_records();
        let id = self.intern(rec, writes, rh.finish128());
        // (a frame that kept no reads: its record is never looked up)
        self.note_lean(id, self.live_records() > live, !keep);
        if self.open.frames.len() == 1 && self.open.step.is_some() {
            // (a call of the top level: the open step's)
            self.open.step_recs.push(id);
        }
        let parent = self.open.frames.last_mut().expect("a parent frame");
        parent.items.push(Item::Call(id));
        parent.cost += total;
        id
    }

    /// Begin a step of the fold at its end (7.17.3): the calls of the top
    /// level until [`Runtime::end_step`] are its records.
    pub fn begin_step(&mut self) -> StepId {
        let id = self.fold.push();
        self.open_step(id);
        id
    }

    /// Run step `id` again in its place: its old definitions and reads
    /// stay live until it ends, and are replaced then.
    pub fn rerun_step(&mut self, id: StepId) {
        self.open_step(id);
    }

    /// The step open now, if any.
    #[must_use]
    pub fn open_step_id(&self) -> Option<StepId> {
        self.open.step.map(|s| s.0)
    }

    /// The step open now and the serial its run began at, if any.
    #[must_use]
    pub fn open_step_serial(&self) -> Option<(StepId, u64)> {
        self.open.step
    }

    /// (debug) Writes made with no call open while a step was.
    #[must_use]
    pub fn root_writes(&self) -> (u64, u64) {
        (self.open.root_writes, self.open.stepless_writes)
    }

    /// How many reads the open step has made from outside it.
    #[must_use]
    pub fn open_step_reads_len(&self) -> usize {
        self.open.step_reads.len()
    }

    /// The addresses the open step has read from outside it so far.
    pub fn open_step_reads(&self) -> impl Iterator<Item = &M::Addr> {
        self.open.step_reads.iter().map(|r| &r.1)
    }

    /// Those reads from the `from`th on.
    pub fn open_step_reads_from(&self, from: usize) -> impl Iterator<Item = &M::Addr> {
        let r = self.open.step_reads.get(from..).unwrap_or_default();
        r.iter().map(|r| &r.1)
    }

    /// The records the open step has made so far (its calls of the top
    /// level that ended).
    #[must_use]
    pub fn open_step_recs(&self) -> &[RecId] {
        &self.open.step_recs
    }

    fn open_step(&mut self, id: StepId) {
        debug_assert!(self.open.step.is_none(), "a step inside a step");
        self.open.serial += 1;
        self.open.step = Some((id, self.open.serial));
        if let Some(st) = self.fold.steps.get_mut(id as usize) {
            st.serial = self.open.serial;
        }
        self.open.step_reads.clear();
        self.open.clear_step_versions();
        self.open.step_recs.clear();
        self.open.step_began = self.open.clock;
        self.open.step_read_at.clear();
        // (a table per step: clearing or walking a large one costs its room)
        self.open.step_wrote_at = Table::new();
    }

    /// Time each step's reads and writes on the engine's clock
    /// ([`StepTimes`], a measurement; [`Runtime::step_times`]).
    pub fn set_timing(&mut self, on: bool) {
        self.open.timing = on;
    }

    /// The engine's clock now (its commands so far), for the steps' times.
    #[inline]
    pub fn set_clock(&mut self, now: u64) {
        self.open.clock = now;
    }

    /// The times of step `id`'s last run, with timing on.
    #[must_use]
    pub fn step_times(&self, id: StepId) -> Option<&StepTimes<M::Addr>> {
        self.step_times.get(id as usize)?.as_ref()
    }

    /// Drop the open step's run (7.17.3's validation: it read a slot
    /// whose definition reaching it was not the one set): its old
    /// definitions and reads stay as they were, and the records of this
    /// run are garbage.
    pub fn abort_step(&mut self) {
        self.open.step = None;
        self.open.step_reads.clear();
        self.open.clear_step_versions();
        self.open.step_recs.clear();
        self.open.step_read_at.clear();
        self.open.step_wrote_at = Table::new();
    }

    /// End the open step: its records' writes are its definitions and its
    /// outside reads make it their slots' reader.
    pub fn end_step(&mut self) -> Option<StepId> {
        let (id, _) = self.open.step.take()?;
        let recs = core::mem::take(&mut self.open.step_recs);
        let reads = core::mem::take(&mut self.open.step_reads);
        self.keep_step_times(id);
        let Runtime {
            recs: arena,
            wa,
            fold,
            ..
        } = self;
        fold.close(
            id,
            recs,
            reads,
            |r| {
                arena[r as usize]
                    .as_ref()
                    .map(|r| wa.get(r.w).iter().map(|w| w.0.clone()).collect())
                    .unwrap_or_default()
            },
            &[],
        );
        self.bare_writes(id);
        Some(id)
    }

    /// With timing on, keep the ending step `id`'s times (its reads' and
    /// last writes'), for whichever way the step ends; else drop them.
    fn keep_step_times(&mut self, id: StepId) {
        if !self.open.timing {
            self.open.step_read_at.clear();
            self.open.step_wrote_at = Table::new();
            return;
        }
        let wrote = core::mem::take(&mut self.open.step_wrote_at)
            .iter()
            .map(|(a, t)| (a.0.clone(), *t))
            .collect();
        let times = StepTimes {
            began: self.open.step_began,
            reads: core::mem::take(&mut self.open.step_read_at),
            wrote,
        };
        let i = id as usize;
        if self.step_times.len() <= i {
            self.step_times.resize_with(i + 1, || None);
        }
        self.step_times[i] = Some(times);
    }

    fn end_step_filtered(&mut self, skip: &[M::Addr]) -> Option<StepId> {
        let (id, _) = self.open.step.take()?;
        let recs = core::mem::take(&mut self.open.step_recs);
        let reads = core::mem::take(&mut self.open.step_reads);
        self.keep_step_times(id);
        let Runtime {
            recs: arena,
            wa,
            fold,
            ..
        } = self;
        fold.close(
            id,
            recs,
            reads,
            |r| {
                arena[r as usize]
                    .as_ref()
                    .map(|r| wa.get(r.w).iter().map(|w| w.0.clone()).collect())
                    .unwrap_or_default()
            },
            skip,
        );
        self.bare_writes(id);
        Some(id)
    }

    /// A read of `loc` at version `ver`, by the running call. Reads of a
    /// slot the call wrote before, and repeated reads, are not recorded.
    #[inline]
    pub fn note_read(&mut self, loc: &Loc<M::Addr>, ver: Version) {
        self.open.note_read_with(loc, || ver);
    }

    /// A *soft* read of `loc` at version `ver` (a local assignment's look
    /// at the value it replaces, saved at the group's start and put back
    /// at its end): the running call's read as [`Runtime::note_read`]
    /// records it, but not the open step's. Whether it would have been the
    /// step's first read of the slot from outside it: the engine then
    /// decides at the step's end ([`Runtime::end_step_soft`]).
    pub fn note_read_soft(&mut self, loc: &Loc<M::Addr>, ver: Version) -> bool {
        self.open.soft = true;
        self.open.soft_new = false;
        self.open.note_read_with(loc, || ver);
        self.open.soft = false;
        if self.open.soft_new && self.open.step_vers.is_some() {
            // (the version it found, the step's entry value, kept for the
            // read the step's end may make of it)
            let a = loc.addr();
            self.open
                .soft_vers
                .entry_by(hash64(a), |k| k.0 == *a, || ByHash(a.clone()), ver);
        }
        self.open.soft_new
    }

    /// Keep (or not) the version of each read of a step from outside it
    /// ([`Runtime::open_step_versions`]): a worker's run, which a commit
    /// in order compares with what its reads find there (DESIGN 3.10,
    /// "Commit"). Every read's version is made while it is on.
    pub fn keep_step_versions(&mut self, on: bool) {
        self.open.step_vers = on.then(Vec::new);
    }

    /// With [`Runtime::keep_step_versions`], the open step's reads from
    /// outside it with the versions they found, in order.
    pub fn open_step_versions(&self) -> impl Iterator<Item = (&M::Addr, Version)> {
        let v = self.open.step_vers.as_deref().unwrap_or_default();
        self.open.step_reads.iter().zip(v).map(|((_, a), v)| (a, *v))
    }

    /// End the open step as [`Runtime::end_step`] does, its soft reads
    /// settled: those in `read` become reads of the step from outside it
    /// (unless it read the slot so already), and the slots in `untouched`
    /// (back at the step's end at the value it began with, or dead) are
    /// not its definitions; its reads of them stay.
    pub fn end_step_soft(&mut self, read: &[M::Addr], untouched: &[M::Addr]) -> Option<StepId> {
        if !read.is_empty() {
            // (a dense slot's stamp says whether the step read it; the
            // step's reads gathered only for another, which a soft read
            // never is: a set of every read of each step that saved an
            // entry value cost 1% of a cold build)
            let mut have: Option<alloc::collections::BTreeSet<&M::Addr>> = None;
            let new: Vec<M::Addr> = read
                .iter()
                .filter(|a| match self.open.dense_step_read(a) {
                    Some(read) => !read,
                    None => !have
                        .get_or_insert_with(|| {
                            self.open.step_reads.iter().map(|(_, a)| a).collect()
                        })
                        .contains(a),
                })
                .cloned()
                .collect();
            for a in &new {
                self.open.force_step_read(a);
            }
        }
        // (a slot left as the step found it is not its definition; its
        // reads stay the step's)
        if untouched.is_empty() {
            return self.end_step();
        }
        self.end_step_filtered(untouched)
    }

    /// Whether the open step has not written dense slot `a`: then the
    /// value it holds is the step's entry value, and `(step, w)` is the
    /// step's serial and the slot's last write before it
    /// ([`Runtime::unwrite`]).
    pub fn entry_write(&mut self, a: &M::Addr) -> Option<(u64, u64)> {
        let step = self.open.step?.1;
        let (f, i) = M::dense(a)?;
        let st = dense_at(&mut self.open.dense, f, i);
        (st.w < step || st.u == step).then_some((step, st.w))
    }

    /// Slot `a` holds again the value it held at the open step's start,
    /// [`Runtime::entry_write`]'s `step` (a group's end put back the value
    /// saved before the step wrote it): until written again, a read of it
    /// is a read of the step's entry value (from outside the step), and the
    /// step does not define it ([`Runtime::end_step_soft`]). The calls'
    /// writes of it stay: the store was written.
    pub fn unwrite(&mut self, a: &M::Addr, step: u64, _w: u64) {
        if self.open.step.map(|s| s.1) != Some(step) {
            return;
        }
        // (the calls keep their writes: the store was written; it is the
        // step that has the slot as it found it)
        if let Some((f, i)) = M::dense(a) {
            dense_at(&mut self.open.dense, f, i).u = step;
        }
    }

    /// [`Runtime::note_read`] with the version made only if the read is
    /// recorded (the first read of `loc` in the call since its last write).
    #[inline]
    pub fn note_read_with(&mut self, loc: &Loc<M::Addr>, ver: impl FnOnce() -> Version) {
        self.open.note_read_with(loc, ver);
    }

    /// The running call wrote `a` (the store holds the new value; an
    /// equal one left its version).
    #[inline]
    pub fn note_write(&mut self, a: &M::Addr) {
        self.open.note_write(a);
    }

    /// An output effect of the running call.
    pub fn note_effect(&mut self, e: M::Effect) {
        if let Some(h) = self.cfg.on_effect {
            h(crate::runtime::EffectKind::Output);
        }
        self.open
            .frames
            .last_mut()
            .expect("a frame")
            .items
            .push(Item::Out(e));
    }

    /// A line stored to `stream` by the running call.
    pub fn note_store(&mut self, stream: &M::Addr, line: M::Val) {
        if let Some(h) = self.cfg.on_effect {
            h(crate::runtime::EffectKind::Store);
        }
        self.open
            .frames
            .last_mut()
            .expect("a frame")
            .items
            .push(Item::Store(stream.clone(), line));
    }

    /// `stream` made to exist by the running call.
    pub fn note_open(&mut self, stream: &M::Addr) {
        if let Some(h) = self.cfg.on_effect {
            h(crate::runtime::EffectKind::Store);
        }
        self.open
            .frames
            .last_mut()
            .expect("a frame")
            .items
            .push(Item::Open(stream.clone()));
    }

    /// `stream` was loaded in this trip.
    pub(crate) fn note_loaded(&mut self, a: &M::Addr) {
        if !self.open.loaded.contains(a) {
            self.open.loaded.push(a.clone());
        }
    }

    /// Account `units` of work to the running call.
    pub fn note_cost(&mut self, units: u64) {
        self.open.frames.last_mut().expect("a frame").own += units;
    }

    /// The value an open call gets when the job ends inside it.
    pub fn set_default_value(&mut self, v: M::Val) {
        self.default_val = Some(v);
    }
}

/// The reads of record `id`'s subtree from outside it, in program order:
/// [`Runtime::walk`]'s, given to `f` (`inside` as there).
fn outside_reads<M: Machine>(
    recs: &[Option<Record<M>>],
    wa: &crate::runtime::Writes<M>,
    id: RecId,
    inside: &mut Table<ByHash<M::Addr>, ()>,
    f: &mut impl FnMut(&Loc<M::Addr>),
) {
    let rec = recs[id as usize].as_ref().expect("a live record");
    for (loc, _) in &rec.reads {
        let a = loc.addr();
        if inside.get_by(hash64(a), |k| k.0 == *a).is_none() {
            f(loc);
        }
    }
    for it in &rec.items {
        match it {
            Item::Wrote(a) => {
                inside.entry_by(hash64(a), |k| k.0 == *a, || ByHash(a.clone()), ());
            }
            Item::Call(c) => {
                outside_reads(recs, wa, *c, inside, f);
                let c = recs[*c as usize].as_ref().expect("a live record");
                for (a, _) in wa.get(c.w) {
                    inside.entry_by(hash64(a), |k| k.0 == *a, || ByHash(a.clone()), ());
                }
            }
            _ => {}
        }
    }
}
