//! A recorded build, invalidation and program-order propagation
//! (`DESIGN.md` §7.3, §7.4).
//!
//! A build is the sequence of region traces of one run, keyed in program
//! order, with two indexes over it: every cell's *backedges* (the regions
//! that read it before writing it) and its *writers* (with the versions
//! written). After an edit, [`Build::rebuild`] walks the old sequence in
//! program order, keeping the new state `S` and the set `D` of cells whose
//! value in `S` differs from the old run's at the same point:
//!
//! - a region none of whose guards is in `D` is *clean*: its writes are
//!   applied (`old exit ⊕ D`) and its effects reused; the cells it writes
//!   leave `D`;
//! - a region with a guard in `D` (found through the backedges, never by
//!   scanning) is *dirty*: it runs again from `S`, recorded at a finer
//!   grain, until the machine reaches the entry of a later old region;
//!   the cells either run wrote are compared by version, and those still
//!   different mark their readers dirty (early cutoff when none are);
//! - once no dirty region is left, the rest of the old run is taken as
//!   it is and its final state is patched with `D` (read-set cutoff).
//!
//! Region boundaries are chosen dynamically from the machine's candidate
//! points: coarse regions on a cold build, fine ones where an edit made the
//! runtime re-execute (edit locality, §7.3).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::exec::Executor;
use crate::hash::{Version, version_of};
use crate::link::{Chunk, link};
use crate::machine::{Machine, Step};
use crate::trace::{FlatRecording, OnForce, Recording, RegionRecorder, Trace};

/// The switches of incremental builds. Every one off gives the same
/// output, only slower.
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)] // independent switches
pub struct Config {
    /// Off: [`Build::rebuild`] records a fresh build from scratch.
    pub incremental: bool,
    /// Off: a dirty region re-executes the whole rest of the run (resume
    /// only, no stop where values agree again).
    pub early_cutoff: bool,
    /// Off: when no dirty region is left, the remaining clean regions are
    /// still replayed one by one instead of patching the old final state.
    pub readset_cutoff: bool,
    /// A region that differs only in an accumulating cell it asked
    /// questions about is clean if its answers hold ([`Machine::derived`];
    /// off: its guard on the cell decides, as for any reader).
    pub derived_guards: bool,
    /// The chooser's region cost on a cold build.
    pub grain: u64,
    /// The chooser's region cost where a rebuild re-executes.
    pub fine_grain: u64,
    /// How the rebuild's grain grows along a re-execution: after each
    /// region cut it is multiplied by this, up to `grain` (1: every
    /// region of `fine_grain`). Regions stay fine where the edit is, where
    /// the next edit is likely, and grow coarse where its effects run
    /// on: a cut costs a snapshot and a hash, which fine regions over a
    /// whole document pay thousands of times.
    pub grow: u64,
    /// The least cost of a region cut at a candidate of level 2 or more
    /// (where a machine prefers a cut whatever the grain: TeX, where an
    /// input file begins or ends, and at a clean point, level 3); 0: no
    /// such cuts. A cold build's region is then an input file read, or
    /// part of one, so a rebuild that re-runs a file read (a `.aux` read
    /// back) stops where the file ends.
    pub file_cut: u64,
    /// After each rebuild, build from scratch and compare the final state
    /// hash and the output (§7.12's sanitizer). Panics on a difference.
    pub sanitize: bool,
    /// Flat recording: first touches of cells with a dense number
    /// ([`crate::Machine::index`]) found in bit sets, written values taken
    /// from the state at a region's end. Off: ordered maps throughout.
    pub flat: bool,
    /// A clock (nanoseconds) for [`Stats`]'s times; none, no times.
    pub clock: Option<fn() -> u64>,
    /// Record, for each span a rebuild re-executes, why its first region
    /// was dirty and whether the re-execution was spurious
    /// ([`Build::audit`]; a diagnostic: the rebuild is the same either
    /// way).
    pub audit: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            incremental: true,
            early_cutoff: true,
            readset_cutoff: true,
            derived_guards: true,
            grain: 128,
            fine_grain: 16,
            grow: 1,
            file_cut: 0,
            sanitize: false,
            flat: true,
            clock: None,
            audit: false,
        }
    }
}

/// What the last build or rebuild did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Whether the rebuild stopped early ([`Build::rebuild_or_stop`]).
    pub stopped: bool,
    /// Regions in the build.
    pub regions: usize,
    /// Old regions re-executed (dirty), and the new regions recorded.
    pub dirty_regions: usize,
    pub recorded_regions: usize,
    /// Cost units executed.
    pub executed_cost: u64,
    /// Of them, those of a span given up by a stop inside it (not kept).
    pub given_up_cost: u64,
    /// Old regions replayed one by one.
    pub replayed_regions: usize,
    /// Spans of clean regions replayed by restoring the old run's state
    /// after them ([`Machine::replay_exit`]); their time is in `replay_ns`.
    pub restores: usize,
    /// Old regions taken as they were by the final cutoff.
    pub reused_regions: usize,
    /// Regions whose keys were renumbered (the order-maintenance fallback).
    pub relabeled: usize,
    /// With a clock, nanoseconds spent re-executing, replaying clean
    /// regions, comparing cells after a re-execution, and in the rest
    /// (the final cutoff, the splice).
    pub run_ns: u64,
    pub replay_ns: u64,
    pub compare_ns: u64,
    pub other_ns: u64,
    /// With a clock, parts of `other_ns`: the start (the new initial
    /// state copied, the changed cells marked), the splice, the old
    /// states dropped.
    pub other_parts_ns: [u64; 3],
    /// With a clock, each re-execution: the old region it began at (its
    /// index), its cost and nanoseconds.
    pub spans: Vec<(usize, u64, u64)>,
}

const SPACING: u64 = 1 << 32;

/// A state that may still be on its way (a saved build's final state,
/// loading on another thread): made when first needed.
pub struct Later<M> {
    value: core::cell::OnceCell<M>,
    make: core::cell::Cell<Option<MakeLater<M>>>,
}

/// What makes a [`Later`] state.
pub type MakeLater<M> = alloc::boxed::Box<dyn FnOnce() -> M + Send>;

impl<M> Later<M> {
    fn now(m: M) -> Self {
        Self {
            value: core::cell::OnceCell::from(m),
            make: core::cell::Cell::new(None),
        }
    }

    fn pending(make: MakeLater<M>) -> Self {
        Self {
            value: core::cell::OnceCell::new(),
            make: core::cell::Cell::new(Some(make)),
        }
    }

    fn get(&self) -> &M {
        self.value.get_or_init(|| {
            let make = self.make.take().expect("a later state is made once");
            make()
        })
    }

    fn into_inner(self) -> M {
        self.get();
        self.value.into_inner().expect("made")
    }

    fn get_mut(&mut self) -> &mut M {
        self.get();
        self.value.get_mut().expect("made")
    }
}

/// One run's regions, in program order, with backedges.
pub struct Build<M: Machine> {
    initial: M,
    final_state: Later<M>,
    seq: BTreeMap<u64, Trace<M>>,
    /// Backedges: the regions that read each cell before writing it.
    readers: CellIndex<M::Cell>,
    /// The regions that write each cell (the version written is the
    /// region's trace's).
    writers: CellIndex<M::Cell>,
    /// Regions by entry position, for re-synchronising after a dirty one.
    entries: BTreeMap<M::Boundary, KeyMap<()>>,
    /// Whether the three indexes above are built (a build makes them
    /// when first needed, [`Build::index`]: a cold run never does).
    indexed: bool,
    /// Rebuilds so far (a trace's `born`).
    generation: u32,
    /// Each cell that differed from the run before, with the last rebuild
    /// in which it did: a region recorded before that holds, in the exit
    /// state the machine kept, an older value (`replay_span`).
    changed: BTreeMap<M::Cell, u32>,
    /// What the last rebuild replaced, not dropped yet: dropping old
    /// regions and states (their machine snapshots) is left to the
    /// driver, for when it waits ([`Build::take_garbage`]).
    garbage: Garbage<M>,
    /// With a clock, each region the last rebuild re-executed from (its
    /// index) and the cells of D it read (for debugging).
    pub why_dirty: Vec<(usize, Vec<M::Cell>)>,
    /// With [`Config::audit`], each span the last rebuild re-executed.
    pub audit: Vec<Audit<M>>,
    /// What the last build or rebuild did.
    pub stats: Stats,
    /// Where a rebuild stopped early ([`Build::rebuild_or_stop`]): the
    /// region it did not reach, and the cells that may differ there
    /// between the state the regions before it lead to and the one the
    /// regions from it on were recorded after. The next rebuild takes
    /// them as changed there.
    frontier: Option<(u64, BTreeSet<M::Cell>)>,
    /// The generations of the last rebuilds that ran to their end (and of
    /// the build before them), oldest first: how recent a region is, for
    /// [`Build::coarsen`], counts in these, not in rebuilds stopped early
    /// (a burst of edits stops many, and what they re-ran is as recent
    /// as the rebuild that settled them).
    settled: Vec<u32>,
    /// Whether the regions' effects, in program order, may differ from
    /// what they were when [`Build::take_effects_changed`] was last called
    /// (a splice compares the effects it takes out with those it puts in:
    /// equal, the output they link to is the same).
    effects_changed: bool,
    /// The keys of the regions put in since [`Build::take_touched`] was
    /// last called (`None`: every region, as after a renumbering).
    touched: Option<BTreeSet<u64>>,
    /// Regions that read something a rename found gone
    /// ([`Build::rename`]): dirty in the next rebuilds until re-run.
    forced: BTreeSet<u64>,
}

/// A value renamed ([`Renaming::value`]).
pub type RenameValue<'a, M> =
    dyn Fn(&<M as Machine>::Cell, &<M as Machine>::Value) -> Option<<M as Machine>::Value> + 'a;

/// How a build is renamed after its input's names moved
/// ([`Build::rename`]): a machine whose cells or boundaries name places
/// in an input (TeX: lines of a file by number) maps them through the
/// input's diff.
pub struct Renaming<'a, M: Machine> {
    /// A cell's new name, the same content under it (its version kept);
    /// `None`: what it named is gone, and a region that read it runs
    /// again.
    pub cell: &'a dyn Fn(&M::Cell) -> Option<M::Cell>,
    /// A boundary's new name (for one inside what changed, a name no new
    /// position has).
    pub boundary: &'a dyn Fn(&M::Boundary) -> M::Boundary,
    /// A value that holds names, renamed (its version is the new
    /// value's); `None`: unchanged.
    pub value: &'a RenameValue<'a, M>,
    /// A guard's version under its new name (by default the same: a
    /// cell whose version is its content's), for a machine whose values
    /// hold names (the stub's lines hold their successor's id).
    pub guard: &'a dyn Fn(&M::Cell, Version) -> Version,
    /// The final state renamed, where it holds names that are not cells
    /// the rebuild patches (the stub's program).
    pub state: &'a dyn Fn(&mut M),
}

/// A span of old regions a rebuild re-executed, audited
/// ([`Config::audit`]): what made its first region dirty, and whether
/// running it again was *spurious*, that is, whether it ended as the old
/// run did. The rebuild compares a span as a whole (from the dirty region
/// to the next old entry the run reaches), not region by region, so the
/// span's other old regions are attributed to the first one's cause: they
/// re-ran because the run had not synchronised yet, not because a guard
/// of theirs failed.
pub struct Audit<M: Machine> {
    /// The first old region's index (in the old run's order).
    pub first: usize,
    /// The old regions' costs, from the first on.
    pub old_costs: Vec<u64>,
    /// What the re-execution cost.
    pub cost: u64,
    /// Whether `S` was at the first region's entry (false: it was dirty
    /// because the run had not synchronised, and `why` is empty).
    pub synced: bool,
    /// The cells of `D` the first region read, with their values at its
    /// entry in the old run and in `S` (both `None` for an accumulating
    /// cell, whose values are deltas).
    pub why: Vec<(M::Cell, Option<Both<M>>)>,
    /// The cells either run wrote whose version at the span's end differs
    /// from the old run's there (those that entered `D`).
    pub differ: Vec<Differ<M>>,
    /// Whether the new regions' effects, in order, are the old span's.
    pub same_effects: bool,
}

/// A cell's value in the old run and in `S` at the same point
/// ([`Audit`]).
pub type Both<M> = (Option<<M as Machine>::Value>, Option<<M as Machine>::Value>);

/// A cell that differs at the end of an audited span ([`Audit::differ`]).
pub struct Differ<M: Machine> {
    pub cell: M::Cell,
    /// Whether it was in `D` at the span's entry already: a difference
    /// from before, which the span may only have carried on.
    pub before: bool,
    /// Its values at the span's end, in the old run and in `S` (`None`
    /// for an accumulating cell).
    pub values: Option<Both<M>>,
    /// For an accumulating cell: whether the new regions added what the
    /// old span's did (each run's additions, in order, added to `S` at
    /// the span's entry give the same version).
    pub same_delta: bool,
}

impl<M: Machine> Audit<M> {
    /// Whether the re-execution ended as the old run did: every cell
    /// either run wrote has its old version at the end, and the effects
    /// are the same. Running it again was wasted for this edit.
    #[must_use]
    pub fn spurious(&self) -> bool {
        self.differ.is_empty() && self.same_effects
    }
}

/// How many generations [`Build::coarsen`] can look back ([`Build::settled`]).
const SETTLED_KEPT: usize = 8;

/// Replaced regions and states, to drop ([`Build::take_garbage`]).
pub struct Garbage<M: Machine> {
    pub traces: Vec<Trace<M>>,
    pub states: Vec<M>,
}

impl<M: Machine> Default for Garbage<M> {
    fn default() -> Self {
        Self {
            traces: Vec::new(),
            states: Vec::new(),
        }
    }
}

/// A clean region `t` ran: the cells it wrote (as the old run did) are no
/// longer different, but for accumulating cells, to which it only added.
fn forget_written<M: Machine>(d: &mut BTreeMap<M::Cell, Option<M::Value>>, t: &Trace<M>) {
    if d.is_empty() {
        return;
    }
    if d.len() * 8 < t.writes.len() {
        d.retain(|c, _| {
            M::accumulates(c) || t.writes.binary_search_by(|(w, _, _)| w.cmp(c)).is_err()
        });
    } else {
        for (c, _, _) in &t.writes {
            if !M::accumulates(c) {
                d.remove(c);
            }
        }
    }
}

/// A region to refine: its key, its successor's, its predecessor, what
/// the regions before it added to accumulating cells, and the region.
type RefineJob<'a, M> = (
    u64,
    Option<u64>,
    Option<&'a Trace<M>>,
    BTreeMap<<M as Machine>::Cell, <M as Machine>::Value>,
    &'a Trace<M>,
);

enum Stop {
    Halted,
    Synced(u64),
    /// `stop` said so after a region was cut: the span is not over.
    Stopped,
}

/// Run `m`, recording regions of about `grain` cost, until it halts or
/// `sync` names the old region whose entry it has reached; flat recording
/// if `flat`. After each region cut (not at a sync), `stop` is asked
/// whether to give up the span ([`Stop::Stopped`]).
fn record_span<M: Machine>(
    m: &mut M,
    grain: Grain,
    flat: bool,
    sync: impl FnMut(&M::Boundary) -> Option<u64>,
    stop: &dyn Fn() -> bool,
) -> (Vec<Trace<M>>, Stop, u64) {
    if flat {
        let rec = FlatRecording::new(m.at());
        record_with(m, grain, sync, stop, rec)
    } else {
        let rec = Recording::new(m.at(), OnForce::Suspend);
        record_with(m, grain, sync, stop, rec)
    }
}

/// A span's grain: its first regions' cost, multiplied by `grow` after
/// each cut, up to `max`.
#[derive(Clone, Copy, Debug)]
struct Grain {
    first: u64,
    grow: u64,
    max: u64,
    /// (`Config::file_cut`)
    file_cut: u64,
}

impl Grain {
    fn fixed(g: u64, file_cut: u64) -> Self {
        Self {
            first: g,
            grow: 1,
            max: g,
            file_cut,
        }
    }
}

fn record_with<M: Machine, R: RegionRecorder<M>>(
    m: &mut M,
    grain: Grain,
    mut sync: impl FnMut(&M::Boundary) -> Option<u64>,
    stop: &dyn Fn() -> bool,
    mut rec: R,
) -> (Vec<Trace<M>>, Stop, u64) {
    let mut out = Vec::new();
    let mut total = 0;
    let mut g = grain.first;
    loop {
        let step = m.step(&mut rec);
        *rec.cost_mut() += 1;
        let cost = *rec.cost_mut();
        match step {
            Step::Continue => {}
            Step::Candidate(level) => {
                if let Some(j) = sync(&m.at()) {
                    total += cost;
                    m.prepare_cut(&mut rec);
                    out.push(rec.cut(m));
                    return (out, Stop::Synced(j), total);
                }
                // The chooser: cut past `file_cut` at a file edge or a
                // clean point (level 2 or 3), once the region is big
                // enough at any candidate of level 1 or more, or anywhere
                // past twice the grain.
                let file = level >= 2 && grain.file_cut > 0 && cost >= grain.file_cut;
                if file || (cost >= g && (level > 0 || cost >= 2 * g)) {
                    total += cost;
                    m.prepare_cut(&mut rec);
                    out.push(rec.cut(m));
                    g = g.saturating_mul(grain.grow).min(grain.max.max(grain.first));
                    if stop() {
                        return (out, Stop::Stopped, total);
                    }
                }
            }
            Step::Halt => {
                total += cost;
                m.prepare_cut(&mut rec);
                out.push(rec.cut(m));
                return (out, Stop::Halted, total);
            }
            Step::Suspended => unreachable!("no holes are planted outside rounds"),
        }
    }
}

impl<M: Machine> Build<M> {
    /// Run `initial` to its end, recording.
    #[must_use]
    pub fn new(initial: M, cfg: &Config) -> Self {
        let mut s = initial.clone();
        let (traces, _, cost) = record_span(
            &mut s,
            Grain::fixed(cfg.grain, cfg.file_cut),
            cfg.flat,
            |_| None,
            &|| false,
        );
        let mut b = Self {
            initial,
            final_state: Later::now(s),
            seq: BTreeMap::new(),
            readers: CellIndex::new(M::index),
            writers: CellIndex::new(M::index),
            entries: BTreeMap::new(),
            indexed: false,
            generation: 0,
            changed: BTreeMap::new(),
            garbage: Garbage::default(),
            why_dirty: Vec::new(),
            audit: Vec::new(),
            frontier: None,
            settled: alloc::vec![0],
            effects_changed: true,
            touched: None,
            forced: BTreeSet::new(),
            stats: Stats::default(),
        };
        let n = traces.len();
        for (i, t) in (1u64..).zip(traces) {
            b.insert(i * SPACING, t);
        }
        b.stats = Stats {
            regions: n,
            recorded_regions: n,
            executed_cost: cost,
            ..Stats::default()
        };
        b
    }

    /// What a build is made of, to keep it outside the process: the
    /// starting and final states, the regions with their keys in program
    /// order, the rebuilds so far and the cells that differed in them
    /// (the indexes are made again, [`Build::index`]).
    #[allow(clippy::type_complexity)]
    pub fn parts(&self) -> (&M, &M, Vec<(u64, &Trace<M>)>, u32, &BTreeMap<M::Cell, u32>) {
        (
            &self.initial,
            self.final_state.get(),
            self.seq.iter().map(|(k, t)| (*k, t)).collect(),
            self.generation,
            &self.changed,
        )
    }

    /// [`Build::from_parts`] with its indexes, made by [`Index::of`] of
    /// the same regions (on another thread, say, while the rest loads).
    #[must_use]
    pub fn from_parts_indexed(
        initial: M,
        final_state: M,
        seq: Vec<(u64, Trace<M>)>,
        generation: u32,
        changed: BTreeMap<M::Cell, u32>,
        index: Index<M>,
    ) -> Self {
        let mut b = Self::from_parts(initial, final_state, seq, generation, changed);
        (b.readers, b.writers, b.entries) = (index.readers, index.writers, index.entries);
        b.indexed = true;
        b
    }

    /// [`Build::from_parts_indexed`] with a final state still to come:
    /// `make` gives it when it is first needed (a rebuild that reaches
    /// the end of the run, or a cutoff that patches it, or a look at it),
    /// so a rebuild can begin while it loads.
    #[must_use]
    pub fn from_parts_later(
        initial: M,
        make: MakeLater<M>,
        seq: Vec<(u64, Trace<M>)>,
        generation: u32,
        changed: BTreeMap<M::Cell, u32>,
        index: Index<M>,
    ) -> Self {
        let mut b = Self::assemble(initial, Later::pending(make), seq, generation, changed);
        (b.readers, b.writers, b.entries) = (index.readers, index.writers, index.entries);
        b.indexed = true;
        b
    }

    /// A build from what [`Build::parts`] gave (the keys strictly
    /// increasing).
    #[must_use]
    pub fn from_parts(
        initial: M,
        final_state: M,
        seq: Vec<(u64, Trace<M>)>,
        generation: u32,
        changed: BTreeMap<M::Cell, u32>,
    ) -> Self {
        Self::assemble(initial, Later::now(final_state), seq, generation, changed)
    }

    fn assemble(
        initial: M,
        final_state: Later<M>,
        seq: Vec<(u64, Trace<M>)>,
        generation: u32,
        changed: BTreeMap<M::Cell, u32>,
    ) -> Self {
        let n = seq.len();
        let mut b = Self {
            initial,
            final_state,
            seq: BTreeMap::new(),
            readers: CellIndex::new(M::index),
            writers: CellIndex::new(M::index),
            entries: BTreeMap::new(),
            indexed: false,
            generation,
            changed,
            garbage: Garbage::default(),
            why_dirty: Vec::new(),
            audit: Vec::new(),
            frontier: None,
            // (a build loaded knows no rebuilds before its own)
            settled: alloc::vec![generation],
            effects_changed: true,
            touched: None,
            forced: BTreeSet::new(),
            stats: Stats::default(),
        };
        for (k, t) in seq {
            b.insert(k, t);
        }
        b.stats.regions = n;
        b
    }

    /// The state the run started from (a rebuild starts from a changed
    /// copy of it).
    pub fn initial(&self) -> &M {
        &self.initial
    }

    /// Rebuilds so far.
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// Whether the last rebuild ran to its end: its final state and
    /// outputs are the inputs' (else it stopped early, and the next
    /// rebuild goes on from where it stopped).
    pub fn settled(&self) -> bool {
        self.frontier.is_none()
    }

    /// Whether the regions' effects, all in program order, may differ
    /// from what they were at the last call (the first call: true). A
    /// driver whose link reads nothing but the effects (TeX's
    /// `effects::link`; not [`Build::output`], which may read the final
    /// state) can take the last link's output again when they do not.
    pub fn take_effects_changed(&mut self) -> bool {
        core::mem::take(&mut self.effects_changed)
    }

    /// The keys of the regions put in or replaced since the last call
    /// (`None`: all of them, as for the first call or after the regions
    /// were renumbered); a driver that keeps something per region (a
    /// link's resolved effects) keeps it for the others.
    pub fn take_touched(&mut self) -> Option<BTreeSet<u64>> {
        self.touched.replace(BTreeSet::new())
    }

    /// The regions with their keys, in program order.
    pub fn keyed_traces(&self) -> impl Iterator<Item = (u64, &Trace<M>)> {
        self.seq.iter().map(|(&k, t)| (k, t))
    }

    /// The state at the end of the run.
    pub fn final_state(&self) -> &M {
        self.final_state.get()
    }

    /// The regions, in program order.
    pub fn traces(&self) -> impl Iterator<Item = &Trace<M>> {
        self.seq.values()
    }

    /// Link the run's output.
    pub fn output<E: Executor>(&self, exec: &E) -> Vec<u8> {
        let chunks: Vec<Chunk<'_, M>> = self
            .seq
            .values()
            .map(|t| Chunk {
                effects: &t.effects,
                env: &[],
                allocs: t.allocs,
            })
            .collect();
        link(&chunks, self.final_state.get(), exec)
    }

    /// Split every region that cost more than `min` into regions of
    /// about `grain`, by running it again from its entry, the regions in
    /// parallel on `exec`: what a driver does while it waits for an edit
    /// (a cold build cuts coarse regions, since each cut costs a snapshot;
    /// a first edit then re-runs a fine one). Output and final state are
    /// unchanged; a region whose re-run does not end where it did stays
    /// as it was. Returns how many regions were split. Only before the
    /// first rebuild (each region's exit state kept must be its own run's:
    /// none is split after).
    pub fn refine<E: Executor>(
        &mut self,
        grain: u64,
        min: u64,
        flat: bool,
        exec: &E,
        clock: Option<fn() -> u64>,
    ) -> (usize, [u64; 3]) {
        if self.generation > 0 {
            return (0, [0; 3]);
        }
        let now = || clock.map_or(0, |c| c());
        let t0 = now();
        // (each region's entry, made on its own thread: the state after its
        // predecessor, from that one's writes, with what the regions
        // before added to accumulating cells)
        let keys: Vec<u64> = self.seq.keys().copied().collect();
        let mut added: BTreeMap<M::Cell, M::Value> = BTreeMap::new();
        let mut jobs: Vec<RefineJob<'_, M>> = Vec::new();
        let mut prev: Option<&Trace<M>> = None;
        for (i, k) in keys.iter().enumerate() {
            let t = &self.seq[k];
            if t.cost > min {
                jobs.push((*k, keys.get(i + 1).copied(), prev, added.clone(), t));
            }
            if let Some(p) = prev {
                for (c, v, _) in &p.writes {
                    if M::accumulates(c) {
                        let v = match added.get(c) {
                            Some(a) => M::combine(c, a, v),
                            None => v.clone(),
                        };
                        added.insert(c.clone(), v);
                    }
                }
            }
            prev = Some(t);
        }
        let t1 = now();
        let initial = &self.initial;
        let results = exec.map(jobs, |(k, next, prev, added, old)| {
            let mut m = initial.clone();
            if let Some(p) = prev {
                for (c, v) in added {
                    m.set(&c, Some(v));
                }
                let none: BTreeMap<M::Cell, Option<M::Value>> = BTreeMap::new();
                if !m.replay_exit(&[p], &mut none.iter()) || m.at() != old.entry {
                    return (k, next, None);
                }
            }
            let last = next.is_none();
            let (traces, stop, cost) = record_span(
                &mut m,
                Grain::fixed(grain, 0),
                flat,
                |b| (!last && *b == old.exit).then_some(0),
                &|| false,
            );
            // (it ended where the region did, in the same state: a
            // position met twice would stop it early)
            let ok = match stop {
                Stop::Synced(_) => !last,
                Stop::Halted => last,
                Stop::Stopped => false,
            } && cost == old.cost
                && old
                    .writes
                    .iter()
                    .all(|(c, _, v)| version_of(&m.get(c)) == *v);
            (k, next, ok.then_some(traces))
        });
        let t2 = now();
        let mut splices = Vec::new();
        for (k, next, traces) in results {
            if let Some(traces) = traces
                && traces.len() > 1
            {
                splices.push((k, next, traces));
            }
        }
        let n = splices.len();
        let mut stats = core::mem::take(&mut self.stats);
        self.splice(splices, &mut stats);
        stats.regions = self.seq.len();
        self.stats = stats;
        (n, [t1 - t0, t2 - t1, now() - t2])
    }

    /// Merge runs of adjacent fine regions that no rebuild recorded in the
    /// last `keep` rebuilds that ran to their end (nor in the rebuilds
    /// stopped early among them) into regions of up to `grain` (a rebuild
    /// leaves fine regions where it re-ran; each keeps a snapshot, so
    /// edits all over a document would otherwise make it fine everywhere,
    /// which is many times a plain run's memory). The merged regions are
    /// their composition: the same output and final state; a later edit
    /// there re-runs one coarse region finely again. Returns how many
    /// regions were merged away.
    pub fn coarsen(&mut self, grain: u64, keep: u32) -> usize {
        // (not while a rebuild stopped early: the frontier is a region)
        if self.frontier.is_some() {
            return 0;
        }
        // (regions recorded since the `keep`-th last rebuild that ran to
        // its end stay fine)
        let Some(&old) = self.settled.iter().rev().nth(keep as usize) else {
            return 0;
        };
        // (a build not indexed, a copy to save, is not indexed for this:
        // a merge asks only for the writers of accumulating cells)
        let acc_writers: Option<BTreeMap<M::Cell, Vec<u64>>> = (!self.indexed).then(|| {
            let mut m: BTreeMap<M::Cell, Vec<u64>> = BTreeMap::new();
            for (k, t) in &self.seq {
                for (c, _, _) in t.writes.iter().filter(|(c, _, _)| M::accumulates(c)) {
                    m.entry(c.clone()).or_default().push(*k);
                }
            }
            m
        });
        let last_writer = |c: &M::Cell, j: u64| -> Option<u64> {
            match &acc_writers {
                Some(m) => m.get(c).and_then(|ks| {
                    let i = ks.partition_point(|k| *k < j);
                    i.checked_sub(1).map(|i| ks[i])
                }),
                None => self.last_writer(c, Some(j)),
            }
        };
        let keys: Vec<u64> = self.seq.keys().copied().collect();
        let mut splices: Vec<(u64, Option<u64>, Vec<Trace<M>>)> = Vec::new();
        let mut i = 0;
        while i < keys.len() {
            let first = &self.seq[&keys[i]];
            if first.born > old || first.cost >= grain {
                i += 1;
                continue;
            }
            let mut acc = crate::trace::Composer::new(first);
            let mut j = i + 1;
            while j < keys.len() {
                let t = &self.seq[&keys[j]];
                if t.born > old || acc.cost() + t.cost > grain {
                    break;
                }
                let k0 = keys[i];
                let entry = |c: &M::Cell| self.known_old_version_after(c, last_writer(c, k0));
                if !acc.push(t, &entry) {
                    break;
                }
                j += 1;
            }
            if j > i + 1 {
                splices.push((keys[i], keys.get(j).copied(), alloc::vec![acc.finish()]));
            }
            i = j;
        }
        let before = self.seq.len();
        let mut stats = core::mem::take(&mut self.stats);
        self.splice(splices, &mut stats);
        stats.regions = self.seq.len();
        self.stats = stats;
        before - self.seq.len()
    }

    /// What rebuilds replaced since the last call, for the caller to drop
    /// (on another thread, or while it waits for an edit).
    pub fn take_garbage(&mut self) -> Garbage<M> {
        core::mem::take(&mut self.garbage)
    }

    /// Rename the build after its input's names moved (DESIGN §7.16.5:
    /// a file's lines renumbered by an insertion or a deletion), so that
    /// its regions mean in the new input what they meant in the old one:
    /// every boundary, every guard's and write's cell, and the values
    /// that hold names ([`Renaming`]). A guard on a cell renamed to
    /// another keeps its version: the content is the same. A cell whose
    /// value was renamed takes the new version everywhere, and a guard on
    /// it takes the version of the renamed write before it (its value at
    /// the region's entry). A region that read a gone cell is dirty in the
    /// next rebuilds until it runs again. The cells that differed in
    /// earlier rebuilds are renamed too, and the indexes are made again.
    pub fn rename(&mut self, r: &Renaming<'_, M>) {
        // (the latest version written of each cell whose value a rename
        // changed, in program order)
        let mut last: BTreeMap<M::Cell, Version> = BTreeMap::new();
        let seq = core::mem::take(&mut self.seq);
        for (k, mut t) in seq {
            t.entry = (r.boundary)(&t.entry);
            t.exit = (r.boundary)(&t.exit);
            let mut guards: BTreeMap<M::Cell, Version> = BTreeMap::new();
            for (c, v) in core::mem::take(&mut t.guards) {
                if let Some(c2) = (r.cell)(&c) {
                    let v2 = last.get(&c2).copied().unwrap_or_else(|| (r.guard)(&c2, v));
                    guards.entry(c2).or_insert(v2);
                } else {
                    self.forced.insert(k);
                }
            }
            t.guards = guards.into_iter().collect();
            let mut writes: Vec<(M::Cell, M::Value, Version)> = Vec::new();
            for (c, v, h) in core::mem::take(&mut t.writes) {
                let Some(c2) = (r.cell)(&c) else {
                    // (a write of what is gone: nothing reads it again)
                    continue;
                };
                let (v2, h2) = if let Some(v2) = (r.value)(&c2, &v) {
                    let h2 = version_of(&Some(v2.clone()));
                    last.insert(c2.clone(), h2);
                    (v2, h2)
                } else {
                    if let Some(l) = last.get_mut(&c2) {
                        *l = h;
                    }
                    (v, h)
                };
                writes.push((c2, v2, h2));
            }
            writes.sort_by(|a, b| a.0.cmp(&b.0));
            t.writes = writes;
            self.seq.insert(k, t);
        }
        self.changed = core::mem::take(&mut self.changed)
            .into_iter()
            .filter_map(|(c, g)| (r.cell)(&c).map(|c2| (c2, g)))
            .collect();
        if let Some((p, cells)) = self.frontier.take() {
            let cells = cells.iter().filter_map(|c| (r.cell)(c)).collect();
            self.frontier = Some((p, cells));
        }
        (r.state)(self.final_state.get_mut());
        // (every region's cells may have moved: the indexes made again)
        self.readers = CellIndex::new(M::index);
        self.writers = CellIndex::new(M::index);
        self.entries = BTreeMap::new();
        self.indexed = false;
        self.touched = None;
        self.effects_changed = true;
        self.index();
    }

    /// Build the backedge, writer and entry indexes, if not yet (a
    /// rebuild needs them; a driver may build them while it waits for an
    /// edit).
    pub fn index(&mut self) {
        if self.indexed {
            return;
        }
        self.indexed = true;
        if self.readers.is_empty() && self.writers.is_empty() && self.entries.is_empty() {
            // (all at once, rather than an insert per guard and write)
            let ix = Index::of(self.seq.iter().map(|(&k, t)| (k, t)));
            (self.readers, self.writers, self.entries) = (ix.readers, ix.writers, ix.entries);
            return;
        }
        // (the same regions, at the same keys: not touched)
        let touched = self.touched.take();
        let seq = core::mem::take(&mut self.seq);
        for (k, t) in seq {
            self.insert(k, t);
        }
        self.touched = touched;
    }

    fn insert(&mut self, key: u64, t: Trace<M>) {
        if let Some(k) = &mut self.touched {
            k.insert(key);
        }
        if !self.indexed {
            self.seq.insert(key, t);
            return;
        }
        for (c, _) in &t.guards {
            self.readers.add(c, key);
        }
        for (c, _, _) in &t.writes {
            self.writers.add(c, key);
        }
        self.entries
            .entry(t.entry.clone())
            .or_default()
            .insert(key, ());
        self.seq.insert(key, t);
    }

    /// Put `t` in the place of region `key`, the old one to the garbage,
    /// updating the indexes by the guards and writes that differ.
    fn replace(&mut self, key: u64, t: Trace<M>) {
        if let Some(k) = &mut self.touched {
            k.insert(key);
        }
        if !self.indexed {
            if let Some(old) = self.seq.insert(key, t) {
                self.garbage.traces.push(old);
            }
            return;
        }
        let old = self.seq.remove(&key).expect("a region in the sequence");
        let (mut i, mut j) = (0, 0);
        let (a, b) = (&old.guards, &t.guards);
        while i < a.len() || j < b.len() {
            let o = match (a.get(i), b.get(j)) {
                (Some((x, _)), Some((y, _))) => x.cmp(y),
                (Some(_), None) => core::cmp::Ordering::Less,
                _ => core::cmp::Ordering::Greater,
            };
            match o {
                core::cmp::Ordering::Less => {
                    self.readers.remove_key(&a[i].0, key);
                    i += 1;
                }
                core::cmp::Ordering::Greater => {
                    self.readers.add(&b[j].0, key);
                    j += 1;
                }
                core::cmp::Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
            }
        }
        let (mut i, mut j) = (0, 0);
        let (a, b) = (&old.writes, &t.writes);
        while i < a.len() || j < b.len() {
            let o = match (a.get(i), b.get(j)) {
                (Some((x, _, _)), Some((y, _, _))) => x.cmp(y),
                (Some(_), None) => core::cmp::Ordering::Less,
                _ => core::cmp::Ordering::Greater,
            };
            match o {
                core::cmp::Ordering::Less => {
                    self.writers.remove_key(&a[i].0, key);
                    i += 1;
                }
                core::cmp::Ordering::Greater => {
                    self.writers.add(&b[j].0, key);
                    j += 1;
                }
                core::cmp::Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
            }
        }
        if old.entry != t.entry {
            if let Some(s) = self.entries.get_mut(&old.entry) {
                s.remove(key);
                if s.is_empty() {
                    self.entries.remove(&old.entry);
                }
            }
            self.entries
                .entry(t.entry.clone())
                .or_default()
                .insert(key, ());
        }
        self.seq.insert(key, t);
        self.garbage.traces.push(old);
    }

    fn remove(&mut self, key: u64) -> Trace<M> {
        let t = self.seq.remove(&key).expect("a region in the sequence");
        if !self.indexed {
            return t;
        }
        for (c, _) in &t.guards {
            self.readers.remove_key(c, key);
        }
        for (c, _, _) in &t.writes {
            self.writers.remove_key(c, key);
        }
        if let Some(s) = self.entries.get_mut(&t.entry) {
            s.remove(key);
            if s.is_empty() {
                self.entries.remove(&t.entry);
            }
        }
        t
    }

    /// Follow `c`'s backedges: mark dirty its readers from `from` up to
    /// and including its next writer (after which, if that writer stays
    /// clean, `c` is no longer different; an accumulating cell stays
    /// different, so all its later readers).
    fn mark(&self, dirty: &mut BTreeSet<u64>, c: &M::Cell, from: u64) {
        let until = if M::accumulates(c) {
            u64::MAX
        } else {
            self.writers
                .get(c)
                .and_then(|w| w.range(from..).next().map(|(k, ())| *k))
                .unwrap_or(u64::MAX)
        };
        if let Some(rs) = self.readers.get(c) {
            dirty.extend(rs.range(from..=until).map(|e| e.0));
        }
    }

    /// The key of the last region before `j` (all of them: `None`) that
    /// writes `c`, by the writers' index.
    fn last_writer(&self, c: &M::Cell, j: Option<u64>) -> Option<u64> {
        let w = self.writers.get(c);
        let last = match j {
            Some(j) => w.and_then(|w| w.range(..j).next_back()),
            None => w.and_then(|w| w.iter().next_back()),
        };
        last.map(|(k, ())| *k)
    }

    /// The version of `c` after region `last` wrote it (`None`: the
    /// starting state's).
    fn old_version_after(&self, c: &M::Cell, last: Option<u64>) -> Version {
        last.map_or_else(
            || version_of(&self.initial.get(c)),
            |k| {
                let w = &self.seq[&k].writes;
                let i = w
                    .binary_search_by(|(x, _, _)| x.cmp(c))
                    .expect("a writer's trace writes the cell");
                w[i].2
            },
        )
    }

    /// The old run's value of `c` just before region `k` (for
    /// [`Build::audit`]).
    fn old_value_before(&self, c: &M::Cell, k: u64) -> Option<M::Value> {
        match self.writers.get(c).and_then(|w| w.range(..k).next_back()) {
            Some((j, ())) => {
                let w = &self.seq[j].writes;
                w.binary_search_by(|(x, _, _)| x.cmp(c))
                    .ok()
                    .map(|i| w[i].1.clone())
            }
            None => self.initial.get(c),
        }
    }

    /// The old run's version of `c` just before region `j` (at the end if
    /// `None`), if the regions know it. An
    /// accumulating cell's version is its whole value, and a region kept
    /// from before the last rebuild in which that value differed recorded
    /// the whole value of its own run, not of the run it is now part of
    /// (the value it added is the same, as it ran the same).
    fn known_old_version_before(&self, c: &M::Cell, j: Option<u64>) -> Option<Version> {
        self.known_old_version_after(c, self.last_writer(c, j))
    }

    /// [`Build::known_old_version_before`] the region whose key is `last`
    /// (`None`: before every region) wrote `c` last.
    fn known_old_version_after(&self, c: &M::Cell, last: Option<u64>) -> Option<Version> {
        if M::accumulates(c)
            && let Some(&since) = self.changed.get(c)
            && last.is_some_and(|k| self.seq[&k].born < since)
        {
            return None;
        }
        Some(self.old_version_after(c, last))
    }

    /// Rebuild after an edit: `new_initial` is the new starting state,
    /// which differs from the old one at most in `changed`.
    ///
    /// # Panics
    ///
    /// With `cfg.sanitize`, if the rebuild differs from a fresh build.
    #[allow(clippy::too_many_lines)] // one propagation loop, kept whole
    pub fn rebuild(&mut self, new_initial: M, changed: &[M::Cell], cfg: &Config) {
        let done = self.rebuild_or_stop(new_initial, changed, cfg, &|| false);
        debug_assert!(done);
    }

    /// [`Build::rebuild`], stopping early if `stop` says so when asked:
    /// between re-executed spans, at an old region's boundary, once one
    /// span re-ran (so a stream of edits still moves on); and inside a
    /// span, after each region it cut, unless the last stop's frontier is
    /// still ahead: the span is then given up (what it ran is dropped, a
    /// newer edit may change it anyway) and the walk stops at the old
    /// region it began at, so that a long span (a page-numbering cascade
    /// over a chapter) does not hold the newer edit back. A stopped
    /// rebuild keeps what it re-ran, takes `new_initial` as its starting
    /// state, and records where it stopped (the frontier): the next
    /// rebuild goes on from there with its own changes, and its final
    /// state and outputs are wanted only from a rebuild that ran to its
    /// end ([`Build::settled`]). `false` if it stopped.
    ///
    /// The regions from the frontier on were recorded after the old run's
    /// states; the cells that differed there, and are not written again
    /// by those regions before a later point, are kept as differing up to
    /// there, whatever their values (not compared with the old run's
    /// writes, which the regions before the frontier no longer hold).
    #[allow(clippy::too_many_lines)] // (one walk over the regions)
    pub fn rebuild_or_stop(
        &mut self,
        new_initial: M,
        changed: &[M::Cell],
        cfg: &Config,
        stop: &dyn Fn() -> bool,
    ) -> bool {
        if !cfg.incremental {
            *self = Self::new(new_initial, cfg);
            return true;
        }
        self.index();
        // (the last rebuild's, if the driver did not take them)
        self.garbage = Garbage::default();
        self.why_dirty.clear();
        self.audit.clear();
        self.generation += 1;
        let generation = self.generation;
        let old_regions = self.seq.len();
        let mut stats = Stats::default();
        let now = || cfg.clock.map_or(0, |c| c());
        let start = now();
        let mut s = new_initial.clone();
        let mut d: BTreeMap<M::Cell, Option<M::Value>> = BTreeMap::new();
        let mut dirty: BTreeSet<u64> = BTreeSet::new();
        // (the regions a rename found reading something gone)
        dirty.extend(self.forced.iter().copied());
        for c in changed {
            let nv = s.get(c);
            if version_of(&nv) != version_of(&self.initial.get(c)) {
                self.mark(&mut dirty, c, 0);
                d.insert(c.clone(), nv);
                self.changed.insert(c.clone(), generation);
            }
        }
        stats.other_parts_ns[0] = now() - start;
        // (first old key replaced, sync key or the end, new traces)
        let mut splices: Vec<(u64, Option<u64>, Vec<Trace<M>>)> = Vec::new();
        let mut cur = self.seq.keys().next().copied();
        let mut final_state = None;
        // Clean regions not applied to S yet (from this key up to the
        // current one): replaying waits until a region runs, and the
        // final cutoff needs none of it.
        let mut pending: Option<u64> = None;
        // (cells that differ, beyond the changed ones: `self.changed`'s
        // once the loop is over, as `replay_span` reads it)
        let mut changed: BTreeSet<M::Cell> = BTreeSet::new();
        // Where the last rebuild stopped, not reached yet; once reached,
        // its key, and its cells that no region from it on wrote yet.
        let mut frontier = self.frontier.take();
        let mut frontier_key: Option<u64> = None;
        let mut unsettled: BTreeSet<M::Cell> = BTreeSet::new();
        let mut stopped_at = None;
        while let Some(c_key) = cur {
            if frontier.is_none() && !splices.is_empty() && stop() {
                stopped_at = Some(c_key);
                break;
            }
            let k = if pending.is_some() || s.at() == self.seq[&c_key].entry {
                dirty.range(c_key..).next().copied()
            } else {
                Some(c_key)
            };
            // (the walk stops at the frontier)
            let k = match &frontier {
                Some((p, _)) if k.is_none_or(|k| k >= *p) => Some(*p),
                _ => k,
            };
            if let Some(k) = k
                && let Some((p, cells)) = frontier.take_if(|(p, _)| *p == k)
            {
                // (the clean regions before it, applied to S first)
                if p > c_key {
                    self.forget_written_in(&mut d, c_key, p);
                    stats.replayed_regions += self.seq.range(c_key..p).count();
                    pending.get_or_insert(c_key);
                }
                if let Some(q) = pending.take() {
                    let t = now();
                    stats.restores += usize::from(self.replay_span(&mut s, q, p, &d));
                    stats.replay_ns += now() - t;
                }
                for c in cells {
                    self.mark(&mut dirty, &c, p);
                    changed.insert(c.clone());
                    unsettled.insert(c.clone());
                    let nv = s.get(&c);
                    d.insert(c, nv);
                }
                frontier_key = Some(p);
                cur = Some(p);
                continue;
            }
            let Some(k) = k else {
                // (an accumulating cell that differs cannot be patched
                // into the old final state, which holds the old run's
                // additions: the rest is replayed then)
                let accumulated = d.keys().any(|c| M::accumulates(c));
                if (d.is_empty() || cfg.readset_cutoff) && !accumulated {
                    // No dirty region is left: the rest of the old run
                    // repeats itself, except for the cells of D nobody
                    // writes again.
                    let mut f = self.final_state.get().clone();
                    for (c, nv) in &d {
                        let again = self
                            .writers
                            .get(c)
                            .is_some_and(|w| w.range(c_key..).next().is_some());
                        if !again {
                            f.set(c, nv.clone());
                        }
                    }
                    stats.reused_regions = self.seq.range(c_key..).count();
                    final_state = Some(f);
                    break;
                }
                // Read-set cutoff off, or an accumulating cell different:
                // replay the rest at once.
                self.forget_written_in(&mut d, c_key, u64::MAX);
                stats.replayed_regions += self.seq.range(c_key..).count();
                let from = pending.take().unwrap_or(c_key);
                stats.restores += usize::from(self.replay_span(&mut s, from, u64::MAX, &d));
                break;
            };
            // The clean regions before k are replayed (later).
            if k > c_key {
                self.forget_written_in(&mut d, c_key, k);
                stats.replayed_regions += self.seq.range(c_key..k).count();
                pending.get_or_insert(c_key);
            }
            let t = &self.seq[&k];
            let synced = pending.is_some() || s.at() == t.entry;
            // (guards of a region kept by its answers, set to what their
            // cells hold now: below)
            let mut refresh: Vec<(M::Cell, Version)> = Vec::new();
            let mut is_dirty = !synced
                || self.forced.contains(&k)
                || if d.len() < t.guards.len() {
                    d.keys().any(|c| t.reads(c))
                } else {
                    t.guards.iter().any(|(c, _)| d.contains_key(c))
                };
            if is_dirty && synced && cfg.readset_cutoff && !self.forced.contains(&k) {
                // (dirty only by accumulating cells: they may have caught
                // up by now, as the glyphs used do when the new ones occur
                // again before)
                let acc: Vec<M::Cell> = d.keys().filter(|c| t.reads(c)).cloned().collect();
                if acc.iter().all(|c| M::accumulates(c)) {
                    if let Some(p) = pending.take() {
                        let t = now();
                        stats.restores += usize::from(self.replay_span(&mut s, p, k, &d));
                        stats.replay_ns += now() - t;
                    }
                    // (its guards hold: the versions it read, not the old
                    // run's writes, which record the sums of their time;
                    // S is at the region's entry now)
                    let t = &self.seq[&k];
                    let own = |c: &M::Cell| {
                        t.guards
                            .binary_search_by(|(g, _)| g.cmp(c))
                            .is_ok_and(|i| version_of(&s.get(c)) == t.guards[i].1)
                    };
                    // (a region that only asked questions about a cell
                    // holds if its answers do, though the cell differs;
                    // `Machine::derived`)
                    let answers = |c: &M::Cell| {
                        cfg.derived_guards
                            && t.answered(c)
                            && t.derived_guards(c)
                                .all(|(g, v)| version_of(&s.get(g)) == *v)
                    };
                    let caught_up: Vec<bool> = acc.iter().map(own).collect();
                    if acc.iter().zip(&caught_up).all(|(c, &up)| up || answers(c)) {
                        // (a cell that caught up is equal again; one whose
                        // answers held still differs, for later readers,
                        // and the region's guard on it takes the value it
                        // has now, so that every guard keeps its real
                        // version)
                        for (c, up) in acc.iter().zip(caught_up) {
                            if up {
                                d.remove(c);
                            } else {
                                refresh.push((c.clone(), version_of(&s.get(c))));
                            }
                        }
                        is_dirty = false;
                    }
                }
            }
            if !is_dirty {
                // Marked, but what made it dirty was overwritten since.
                forget_written(&mut d, t);
                if !refresh.is_empty()
                    && let Some(t) = self.seq.get_mut(&k)
                {
                    for (c, v) in refresh.drain(..) {
                        if let Ok(i) = t.guards.binary_search_by(|(g, _)| g.cmp(&c)) {
                            t.guards[i].1 = v;
                        }
                    }
                }
                pending.get_or_insert(k);
                stats.replayed_regions += 1;
                cur = self.seq.range(k + 1..).next().map(|(k, _)| *k);
                continue;
            }
            // Dirty: execute from S until the entry of a later old region.
            if cfg.clock.is_some() {
                // (what made it dirty: the cells of D it reads)
                let why: Vec<M::Cell> = t
                    .guards
                    .iter()
                    .filter(|(c, _)| d.contains_key(c))
                    .map(|(c, _)| c.clone())
                    .collect();
                self.why_dirty.push((self.seq.range(..k).count(), why));
            }
            if let Some(p) = pending.take() {
                let t = now();
                stats.restores += usize::from(self.replay_span(&mut s, p, k, &d));
                stats.replay_ns += now() - t;
            }
            // (the audit: the cells of D the region read, with their
            // values at its entry in both runs; S is there now)
            let audit_why = cfg.audit.then(|| {
                self.seq[&k]
                    .guards
                    .iter()
                    .filter(|(c, _)| d.contains_key(c))
                    .map(|(c, _)| {
                        let values =
                            (!M::accumulates(c)).then(|| (self.old_value_before(c, k), s.get(c)));
                        (c.clone(), values)
                    })
                    .collect::<Vec<_>>()
            });
            // (and the state there, to add each run's deltas of an
            // accumulating cell to)
            let audit_entry = cfg.audit.then(|| s.clone());
            let t = now();
            let entries = &self.entries;
            let grain = Grain {
                first: cfg.fine_grain,
                grow: cfg.grow.max(1),
                max: cfg.grain,
                file_cut: cfg.file_cut,
            };
            // (a stop inside the span gives it up: what it ran is not kept,
            // and the walk stops at its first region, as at the top of the
            // loop; not while the last stop's frontier is ahead, whose cells
            // this span's comparison takes up)
            let may_stop = frontier.is_none();
            let span_stop = || may_stop && stop();
            let (traces, stop_, cost) = record_span(
                &mut s,
                grain,
                cfg.flat,
                |b| {
                    if !cfg.early_cutoff {
                        return None;
                    }
                    entries
                        .get(b)
                        .and_then(|ks| ks.range(k + 1..).next().map(|e| e.0))
                },
                &span_stop,
            );
            stats.executed_cost += cost;
            stats.run_ns += now() - t;
            if matches!(stop_, Stop::Stopped) {
                stats.given_up_cost += cost;
                stopped_at = Some(k);
                break;
            }
            if cfg.clock.is_some() {
                stats
                    .spans
                    .push((self.seq.range(..k).count(), cost, now() - t));
            }
            let t = now();
            let j = match stop_ {
                Stop::Synced(j) => Some(j),
                Stop::Halted => None,
                Stop::Stopped => unreachable!("a span given up is not spliced"),
            };
            let old_span: Vec<&Trace<M>> = match j {
                Some(j) => self.seq.range(k..j).map(|(_, t)| t).collect(),
                None => self.seq.range(k..).map(|(_, t)| t).collect(),
            };
            stats.dirty_regions += old_span.len();
            stats.recorded_regions += traces.len();
            let audit_span = cfg.audit.then(|| {
                let same = traces
                    .iter()
                    .flat_map(|t| &t.effects)
                    .eq(old_span.iter().flat_map(|t| &t.effects));
                (old_span.iter().map(|t| t.cost).collect::<Vec<u64>>(), same)
            });
            let mut differ: Vec<Differ<M>> = Vec::new();
            // Compare what either run wrote: equal cells leave D, the rest
            // enter it and mark their readers (early cutoff when none).
            let mut touched: BTreeSet<&M::Cell> = BTreeSet::new();
            for t in &traces {
                touched.extend(t.writes.iter().map(|(c, _, _)| c));
            }
            for t in &old_span {
                touched.extend(t.writes.iter().map(|(c, _, _)| c));
            }
            // (a span that ran past the frontier: its cells, compared as
            // those reached there)
            let crossed = match (&frontier, j) {
                (Some((p, _)), j) if j.is_none_or(|j| j > *p) => frontier.take(),
                _ => None,
            };
            if let Some((p, cells)) = &crossed {
                frontier_key = Some(*p);
                unsettled.extend(cells.iter().cloned());
                touched.extend(cells.iter());
            }
            for c in touched {
                let nv = s.get(c);
                // (a frontier cell that no region from the frontier on
                // wrote before j: the old run's value there is not known)
                let settled = !unsettled.contains(c)
                    || frontier_key.is_some_and(|p| {
                        self.writers
                            .get(c)
                            .is_some_and(|w| w.range(p..j.unwrap_or(u64::MAX)).next().is_some())
                    });
                if settled {
                    unsettled.remove(c);
                }
                if settled && self.known_old_version_before(c, j) == Some(version_of(&nv)) {
                    d.remove(c);
                } else {
                    if let Some(j) = j {
                        self.mark(&mut dirty, c, j);
                    }
                    if cfg.audit {
                        let write_of = |t: &Trace<M>| {
                            t.writes
                                .binary_search_by(|(x, _, _)| x.cmp(c))
                                .ok()
                                .map(|i| t.writes[i].1.clone())
                        };
                        let acc = M::accumulates(c);
                        // (the deltas each run's regions added, however
                        // they were cut, added to the state at the entry)
                        let sum = |e: &M, ws: Vec<M::Value>| {
                            let mut x = e.clone();
                            for v in ws {
                                x.set(c, Some(v));
                            }
                            version_of(&x.get(c))
                        };
                        let same_delta = acc
                            && audit_entry.as_ref().is_some_and(|e| {
                                sum(e, traces.iter().filter_map(write_of).collect())
                                    == sum(e, old_span.iter().filter_map(|t| write_of(t)).collect())
                            });
                        differ.push(Differ {
                            cell: c.clone(),
                            before: d.contains_key(c),
                            values: (!acc).then(|| {
                                (self.old_value_before(c, j.unwrap_or(u64::MAX)), nv.clone())
                            }),
                            same_delta,
                        });
                    }
                    changed.insert(c.clone());
                    d.insert(c.clone(), nv);
                }
            }
            let mut traces = traces;
            for t in &mut traces {
                t.born = generation;
            }
            splices.push((k, j, traces));
            stats.compare_ns += now() - t;
            if let (Some(why), Some((old_costs, same_effects))) = (audit_why, audit_span) {
                self.audit.push(Audit {
                    first: self.seq.range(..k).count(),
                    old_costs,
                    cost,
                    synced,
                    why,
                    differ,
                    same_effects,
                });
            }
            cur = j;
        }
        if let Some(at) = stopped_at {
            // Stopped: what re-ran is kept; the rest waits for the next
            // rebuild, from the frontier.
            for c in changed {
                self.changed.insert(c, generation);
            }
            let t = now();
            let key = self.splice_keeping(splices, &mut stats, Some(at));
            stats.other_parts_ns[1] = now() - t;
            debug_assert!(key.is_some(), "the region a rebuild stopped at is kept");
            self.frontier = Some((key.unwrap_or(at), d.into_keys().collect()));
            let old_initial = core::mem::replace(&mut self.initial, new_initial);
            self.garbage.states.push(old_initial);
            stats.regions = self.seq.len();
            stats.stopped = true;
            self.stats = stats;
            return false;
        }
        if final_state.is_none()
            && let Some(p) = pending.take()
        {
            let t = now();
            stats.restores += usize::from(self.replay_span(&mut s, p, u64::MAX, &d));
            stats.replay_ns += now() - t;
        }
        let s = final_state.unwrap_or(s);
        for c in changed {
            self.changed.insert(c, generation);
        }
        let t = now();
        self.splice(splices, &mut stats);
        stats.other_parts_ns[1] = now() - t;
        let t = now();
        let old_initial = core::mem::replace(&mut self.initial, new_initial);
        let old_final = core::mem::replace(&mut self.final_state, Later::now(s)).into_inner();
        self.garbage.states.extend([old_initial, old_final]);
        stats.other_parts_ns[2] = now() - t;
        stats.regions = self.seq.len();
        if self.settled.len() == SETTLED_KEPT {
            self.settled.remove(0);
        }
        self.settled.push(generation);
        stats.other_ns =
            (now() - start).saturating_sub(stats.run_ns + stats.replay_ns + stats.compare_ns);
        debug_assert!(
            stats.reused_regions + stats.replayed_regions + stats.dirty_regions <= old_regions
        );
        self.stats = stats;
        if cfg.sanitize {
            self.sanitize(cfg);
        }
        true
    }

    /// Replay the regions `[from, to)` on `s` at once, `d` the cells whose
    /// values differ from the old run's after them: the old run's state
    /// there patched with `d`, if the machine can take it
    /// ([`Machine::replay_exit`]); else the last value each cell was
    /// written, applied in cell order, then the position of the last one.
    /// Whether it restored the old run's state.
    fn replay_span(
        &self,
        s: &mut M,
        from: u64,
        to: u64,
        d: &BTreeMap<M::Cell, Option<M::Value>>,
    ) -> bool {
        let span: Vec<&Trace<M>> = self.seq.range(from..to).map(|(_, t)| t).collect();
        match span.as_slice() {
            [] => {}
            [t] => t.apply(s, &[]),
            [.., last]
                if s.replay_exit(
                    &span,
                    &mut self.exit_patch(s, (from, to), last.born, d).iter(),
                ) =>
            {
                return true;
            }
            [.., last] => {
                let mut writes: BTreeMap<&M::Cell, &M::Value> = BTreeMap::new();
                let mut adds: Vec<(&M::Cell, &M::Value)> = Vec::new();
                for t in &span {
                    for (c, v, _) in &t.writes {
                        if M::accumulates(c) {
                            adds.push((c, v));
                        } else {
                            writes.insert(c, v);
                        }
                    }
                }
                let writes: Vec<(&M::Cell, &M::Value)> = writes.into_iter().collect();
                s.set_all(&writes);
                for (c, v) in adds {
                    s.set(c, Some(v.clone()));
                }
                s.seek(&last.exit);
            }
        }
        false
    }

    /// [`forget_written`] for the clean regions `[from, to)`, by the
    /// writers index.
    fn forget_written_in(&self, d: &mut BTreeMap<M::Cell, Option<M::Value>>, from: u64, to: u64) {
        d.retain(|c, _| {
            M::accumulates(c)
                || self
                    .writers
                    .get(c)
                    .is_none_or(|w| w.range(from..to).next().is_none())
        });
    }

    /// What to patch the exit state kept with the last region of the span
    /// `[from, to)` (recorded in rebuild `born`) with: `d`, and each cell
    /// that differed in a later rebuild than `born` (the state kept holds
    /// an older value): its last write in the span, else its value in
    /// `s`, the span's entry. The last write is found by the writers
    /// index, not by looking through the span's regions (cells that
    /// differed accumulate over a session, and spans grow with the
    /// document).
    fn exit_patch(
        &self,
        s: &M,
        (from, to): (u64, u64),
        born: u32,
        d: &BTreeMap<M::Cell, Option<M::Value>>,
    ) -> BTreeMap<M::Cell, Option<M::Value>> {
        let last_key = self.seq.range(from..to).next_back().map(|(k, _)| *k);
        let mut patch = d.clone();
        for (c, g) in &self.changed {
            if *g <= born || M::accumulates(c) || patch.contains_key(c) {
                continue;
            }
            let writer = self
                .writers
                .get(c)
                .and_then(|w| w.range(from..to).next_back().map(|(k, ())| *k));
            if writer.is_some() && writer == last_key {
                // (the state kept is the last region's: its own write)
                continue;
            }
            let written = writer.map(|k| {
                let w = &self.seq[&k].writes;
                let i = w
                    .binary_search_by(|(x, _, _)| x.cmp(c))
                    .expect("a writer's trace writes the cell");
                w[i].1.clone()
            });
            let v = written.or_else(|| s.get(c));
            patch.insert(c.clone(), v);
        }
        patch
    }

    /// Replace each old span `[from, to)` with its new traces, keys chosen
    /// in the gap (order maintenance); renumber everything if a gap is too
    /// small.
    fn splice(&mut self, splices: Vec<(u64, Option<u64>, Vec<Trace<M>>)>, stats: &mut Stats) {
        self.splice_keeping(splices, stats, None);
    }

    /// [`Build::splice`], and the key region `keep` (an old region no
    /// splice replaces) has after it.
    fn splice_keeping(
        &mut self,
        splices: Vec<(u64, Option<u64>, Vec<Trace<M>>)>,
        stats: &mut Stats,
        keep: Option<u64>,
    ) -> Option<u64> {
        // (the effects taken out against those put in, in program order: a
        // rebuild whose re-run regions made the same output, as after a
        // `\label`, links to the same files; costs what was replaced)
        if !self.effects_changed {
            self.effects_changed = splices.iter().any(|(from, to, traces)| {
                let old = match to {
                    Some(to) => self.seq.range(*from..*to),
                    None => self.seq.range(*from..),
                };
                !old.flat_map(|(_, t)| t.effects.iter())
                    .eq(traces.iter().flat_map(|t| t.effects.iter()))
            });
        }
        // (the indexes' changes applied at once, at the end)
        self.readers.begin();
        self.writers.begin();
        let kept = self.splice_staged(splices, stats, keep);
        self.readers.flush();
        self.writers.flush();
        kept
    }

    fn splice_staged(
        &mut self,
        splices: Vec<(u64, Option<u64>, Vec<Trace<M>>)>,
        stats: &mut Stats,
        keep: Option<u64>,
    ) -> Option<u64> {
        let room = |from: u64, to: Option<u64>, n: usize| to.unwrap_or(u64::MAX) - from >= n as u64;
        // (a region replaced ran again: no longer forced)
        for (from, to, _) in &splices {
            self.forced
                .retain(|k| *k < *from || to.is_some_and(|to| *k >= to));
        }
        if splices.iter().all(|(f, t, tr)| room(*f, *t, tr.len())) {
            for (from, to, traces) in splices {
                let old: Vec<u64> = match to {
                    Some(to) => self.seq.range(from..to).map(|(k, _)| *k).collect(),
                    None => self.seq.range(from..).map(|(k, _)| *k).collect(),
                };
                if old.len() == traces.len() {
                    // (as many regions as before: each takes its old key,
                    // and the indexes change by what differs)
                    for (k, t) in old.into_iter().zip(traces) {
                        self.replace(k, t);
                    }
                    continue;
                }
                for k in old {
                    let t = self.remove(k);
                    self.garbage.traces.push(t);
                }
                let step = (to.unwrap_or(u64::MAX) - from) / traces.len() as u64;
                let step = step.min(SPACING);
                for (i, t) in (0u64..).zip(traces) {
                    self.insert(from + i * step, t);
                }
            }
            return keep.filter(|k| self.seq.contains_key(k));
        }
        let mut all = Vec::with_capacity(self.seq.len());
        let mut kept = None;
        // (the forced regions kept, by their place in `all`)
        let mut forced = Vec::new();
        let mut old = core::mem::take(&mut self.seq).into_iter().peekable();
        for (from, to, traces) in splices {
            while let Some((k, t)) = old.next_if(|(k, _)| *k < from) {
                if Some(k) == keep {
                    kept = Some(all.len());
                }
                if self.forced.contains(&k) {
                    forced.push(all.len());
                }
                all.push(t);
            }
            while let Some((_, t)) = old.next_if(|(k, _)| to.is_none_or(|to| *k < to)) {
                self.garbage.traces.push(t);
            }
            all.extend(traces);
        }
        for (k, t) in old {
            if Some(k) == keep {
                kept = Some(all.len());
            }
            if self.forced.contains(&k) {
                forced.push(all.len());
            }
            all.push(t);
        }
        self.forced = forced
            .into_iter()
            .map(|i| (i as u64 + 1) * SPACING)
            .collect();
        self.readers.clear();
        self.writers.clear();
        self.entries.clear();
        // (every key new)
        self.touched = None;
        stats.relabeled = all.len();
        for (i, t) in (1u64..).zip(all) {
            self.insert(i * SPACING, t);
        }
        kept.map(|i| (i as u64 + 1) * SPACING)
    }

    /// Panics unless the regions chain from the initial state, each
    /// one's guards hold where the one before ends, and replaying them all
    /// gives a state of digest `want` (the sanitizer's, and a check of
    /// [`Build::refine`]).
    ///
    /// # Panics
    ///
    /// On the first difference found.
    pub fn check_replay(&self, want: &Version) {
        let mut s = self.initial.clone();
        for (i, t) in self.seq.values().enumerate() {
            assert_eq!(
                s.at(),
                t.entry,
                "sanitizer: regions do not chain (region {i})"
            );
            if let Some((c, v)) = t.guards.iter().find(|(c, v)| version_of(&s.get(c)) != *v) {
                panic!(
                    "sanitizer: region {i}'s guard fails on replay: {c:?} was {v:?}, is {:?}",
                    version_of(&s.get(c))
                );
            }
            t.apply(&mut s, &[]);
        }
        assert_eq!(
            s.digest(),
            *want,
            "sanitizer: replaying the traces does not give the final state"
        );
    }

    fn sanitize(&self, cfg: &Config) {
        // (the regions first: a guard that fails names the region)
        self.check_replay(&self.final_state.get().digest());
        let fresh = Self::new(self.initial.clone(), cfg);
        assert_eq!(
            self.final_state.get().digest(),
            fresh.final_state.get().digest(),
            "sanitizer: the rebuild's final state differs from a fresh build's"
        );
        self.check_replay(&fresh.final_state.get().digest());
        // (what the machine reproduces on purpose: `Machine::comparable_output`)
        assert_eq!(
            M::comparable_output(self.output(&crate::exec::Sequential)),
            M::comparable_output(fresh.output(&crate::exec::Sequential)),
            "sanitizer: the rebuild's output differs from a fresh build's"
        );
    }
}

/// Each cell's region keys (the regions that read it, or that write it),
/// held by a small id per cell: the machine's dense number for the cell
/// ([`Machine::index`]: registers, eqtb words) looked up in pages, else
/// the cell interned by its hash (open addressing). A lookup is a page
/// read or a hash of the cell and one comparison, where a B-tree keyed by
/// cells compared a cell against a dozen others, and a line cell compares
/// its path's bytes; a cut adds some 860 guards. Ids are never shown or
/// saved: the cells are.
pub(crate) struct CellIndex<C> {
    /// The dense number of a cell, if it has one.
    number: fn(&C) -> Option<u32>,
    /// Keys by id.
    maps: Vec<KeyMap<()>>,
    /// Dense number to id + 1, in pages of `PAGE` allocated when used.
    pages: Vec<Option<alloc::boxed::Box<[u32]>>>,
    /// The cells without a dense number, their hashes and ids, and the
    /// table of their places + 1 (a power of two long, at most half full).
    far: Vec<(C, u64, u32)>,
    table: Vec<u32>,
    /// While a splice runs ([`CellIndex::begin`]): the changes, by id,
    /// key and whether added, applied at once by [`CellIndex::flush`] (a
    /// cell most regions read, as `Rest`, then takes one merge of its
    /// keys, not a shift of its keys for each region put in).
    staged: Option<Vec<(u32, u64, bool)>>,
}

const PAGE: usize = 1024;

/// The place in a table of `mask + 1` (a power of two) that hash `h`
/// starts from: its low bits.
#[allow(clippy::cast_possible_truncation)] // (the low bits, meant)
fn slot(h: u64, mask: usize) -> usize {
    (h as usize) & mask
}

impl<C: Clone + Eq + core::hash::Hash> CellIndex<C> {
    fn new(number: fn(&C) -> Option<u32>) -> Self {
        Self {
            number,
            maps: Vec::new(),
            pages: Vec::new(),
            far: Vec::new(),
            table: Vec::new(),
            staged: None,
        }
    }

    fn is_empty(&self) -> bool {
        self.maps.is_empty()
    }

    fn clear(&mut self) {
        let staging = self.staged.is_some();
        *self = Self::new(self.number);
        if staging {
            self.begin();
        }
    }

    /// Stage the changes from now on, until [`CellIndex::flush`] (no
    /// lookups meanwhile).
    fn begin(&mut self) {
        self.staged.get_or_insert_with(Vec::new);
    }

    /// Apply the changes staged since [`CellIndex::begin`]: each cell's
    /// keys are its keys less those removed, with those added (a key is
    /// never added and then removed while staged: a splice removes a
    /// span's old regions, then puts in its new ones).
    fn flush(&mut self) {
        let Some(staged) = self.staged.take() else {
            return;
        };
        if staged.is_empty() {
            return;
        }
        // (by id, a counting sort)
        let n = self.maps.len();
        let mut start = alloc::vec![0usize; n + 1];
        for &(id, _, _) in &staged {
            start[id as usize + 1] += 1;
        }
        for i in 0..n {
            start[i + 1] += start[i];
        }
        let mut fill = start.clone();
        let mut ops = alloc::vec![(0u64, false); staged.len()];
        for &(id, k, added) in &staged {
            let at = &mut fill[id as usize];
            ops[*at] = (k, added);
            *at += 1;
        }
        drop(staged);
        let (mut rem, mut add) = (Vec::new(), Vec::new());
        for id in 0..n {
            let (lo, hi) = (start[id], start[id + 1]);
            if lo == hi {
                continue;
            }
            rem.clear();
            add.clear();
            for &(k, added) in &ops[lo..hi] {
                if added { add.push(k) } else { rem.push(k) }
            }
            rem.sort_unstable();
            add.sort_unstable();
            add.dedup();
            let old = core::mem::take(&mut self.maps[id].0);
            let mut out = Vec::with_capacity(old.len() + add.len());
            let (mut r, mut a) = (0, 0);
            for (k, ()) in old {
                while a < add.len() && add[a] < k {
                    out.push((add[a], ()));
                    a += 1;
                }
                while r < rem.len() && rem[r] < k {
                    r += 1;
                }
                if a < add.len() && add[a] == k {
                    out.push((k, ()));
                    a += 1;
                } else if r >= rem.len() || rem[r] != k {
                    out.push((k, ()));
                }
            }
            out.extend(add[a..].iter().map(|&k| (k, ())));
            if out.capacity() > 2 * out.len() + 4 {
                out.shrink_to_fit();
            }
            self.maps[id].0 = out;
        }
    }

    fn hash(c: &C) -> u64 {
        let mut h = Fx(0);
        c.hash(&mut h);
        h.0
    }

    /// The id of `c`, if it has one.
    fn id(&self, c: &C) -> Option<u32> {
        if let Some(n) = (self.number)(c) {
            let n = n as usize;
            let page = self.pages.get(n / PAGE)?.as_ref()?;
            return page[n % PAGE].checked_sub(1);
        }
        if self.table.is_empty() {
            return None;
        }
        let h = Self::hash(c);
        let mask = self.table.len() - 1;
        let mut i = slot(h, mask);
        loop {
            let slot = self.table[i];
            if slot == 0 {
                return None;
            }
            let (x, xh, id) = &self.far[slot as usize - 1];
            if *xh == h && x == c {
                return Some(*id);
            }
            i = (i + 1) & mask;
        }
    }

    /// The id of `c`, given one if it has none.
    fn id_or_new(&mut self, c: &C) -> u32 {
        if let Some(id) = self.id(c) {
            return id;
        }
        let id = u32::try_from(self.maps.len()).expect("fewer than 2^32 cells");
        self.maps.push(KeyMap::default());
        if let Some(n) = (self.number)(c) {
            let n = n as usize;
            if self.pages.len() <= n / PAGE {
                self.pages.resize_with(n / PAGE + 1, || None);
            }
            let page = self.pages[n / PAGE]
                .get_or_insert_with(|| alloc::vec![0u32; PAGE].into_boxed_slice());
            page[n % PAGE] = id + 1;
            return id;
        }
        if 2 * (self.far.len() + 1) > self.table.len() {
            // (grown to twice, the places put again by their hashes)
            let len = (2 * self.table.len()).max(64);
            self.table = alloc::vec![0u32; len];
            for (place, (_, h, _)) in self.far.iter().enumerate() {
                let mut i = slot(*h, len - 1);
                while self.table[i] != 0 {
                    i = (i + 1) & (len - 1);
                }
                self.table[i] = u32::try_from(place + 1).unwrap_or(u32::MAX);
            }
        }
        let h = Self::hash(c);
        let mask = self.table.len() - 1;
        let mut i = slot(h, mask);
        while self.table[i] != 0 {
            i = (i + 1) & mask;
        }
        self.far.push((c.clone(), h, id));
        self.table[i] = u32::try_from(self.far.len()).unwrap_or(u32::MAX);
        id
    }

    /// The keys of `c` (none, or empty, if no region has it).
    fn get(&self, c: &C) -> Option<&KeyMap<()>> {
        debug_assert!(self.staged.is_none(), "a lookup while changes are staged");
        self.id(c).map(|id| &self.maps[id as usize])
    }

    /// Region `key` has `c`.
    fn add(&mut self, c: &C, key: u64) {
        let id = self.id_or_new(c);
        match &mut self.staged {
            Some(s) => s.push((id, key, true)),
            None => self.maps[id as usize].insert(key, ()),
        }
    }

    /// Region `key` no longer has `c`.
    fn remove_key(&mut self, c: &C, key: u64) {
        if let Some(id) = self.id(c) {
            match &mut self.staged {
                Some(s) => s.push((id, key, false)),
                None => self.maps[id as usize].remove(key),
            }
        }
    }
}

/// A small ordered map from region keys, as a sorted vector: an index's
/// entry per cell (most cells have a few readers or writers, where a
/// B-tree's node would cost ten times the space).
///
/// (`grouped` builds a map of them from pairs in any order.)
#[derive(Clone, Debug)]
struct KeyMap<V>(Vec<(u64, V)>);

/// A build's indexes (readers, writers and entries by region key), made
/// apart from it ([`Build::from_parts_indexed`]).
pub struct Index<M: Machine> {
    readers: CellIndex<M::Cell>,
    writers: CellIndex<M::Cell>,
    entries: BTreeMap<M::Boundary, KeyMap<()>>,
}

impl<M: Machine> Index<M> {
    /// The indexes of `seq`, in increasing key order.
    pub fn of<'a>(seq: impl Iterator<Item = (u64, &'a Trace<M>)>) -> Self
    where
        M: 'a,
    {
        // (keys in increasing order: each cell's are appended)
        let (mut readers, mut writers) = (CellIndex::new(M::index), CellIndex::new(M::index));
        let mut e = Vec::new();
        for (k, t) in seq {
            for (c, _) in &t.guards {
                readers.add(c, k);
            }
            for (c, _, _) in &t.writes {
                writers.add(c, k);
            }
            e.push((t.entry.clone(), k));
        }
        Self {
            readers,
            writers,
            entries: grouped(&e, |_| None),
        }
    }
}

/// `pairs` (in increasing key order) as a map from each item to its
/// keys. Items `dense` numbers are grouped by number (a counting sort);
/// the others by their hashes, integers that sort fast; only the distinct
/// items are sorted by their order.
fn grouped<K: Ord + core::hash::Hash + Clone>(
    pairs: &[(K, u64)],
    dense: impl Fn(&K) -> Option<u32>,
) -> BTreeMap<K, KeyMap<()>> {
    let mut out: Vec<(K, KeyMap<()>)> = Vec::new();
    // Numbered items: counted, then each number's keys in order.
    let mut slots: Vec<Option<u32>> = pairs.iter().map(|(c, _)| dense(c)).collect();
    let mut n = slots.iter().flatten().max().map_or(0, |&m| m as usize + 1);
    if n > 8 * pairs.len() + 4096 {
        // (numbered too sparsely to count: all by hash)
        slots.fill(None);
        n = 0;
    }
    let mut count = alloc::vec![0u32; n];
    for s in slots.iter().flatten() {
        count[*s as usize] += 1;
    }
    let mut first: Vec<Option<usize>> = alloc::vec![None; n];
    let mut groups: Vec<KeyMap<()>> = count
        .iter()
        .map(|&c| KeyMap(Vec::with_capacity(c as usize)))
        .collect();
    for (i, s) in slots.iter().enumerate() {
        if let Some(s) = s {
            let s = *s as usize;
            let k = pairs[i].1;
            let g = &mut groups[s].0;
            if g.last().is_none_or(|l| l.0 != k) {
                g.push((k, ()));
            }
            first[s].get_or_insert(i);
        }
    }
    for (s, g) in groups.into_iter().enumerate() {
        if let Some(i) = first[s] {
            out.push((pairs[i].0.clone(), g));
        }
    }
    let numbered = out.len();
    // The others, by hash.
    let mut by_hash: Vec<(u64, u32)> = pairs
        .iter()
        .zip(&slots)
        .enumerate()
        .filter(|(_, (_, s))| s.is_none())
        .map(|(i, ((c, _), _))| {
            let mut h = Fx(0);
            c.hash(&mut h);
            (h.0, u32::try_from(i).unwrap_or(u32::MAX))
        })
        .collect();
    by_hash.sort_unstable();
    let mut run_start = numbered;
    for (n, &(h, i)) in by_hash.iter().enumerate() {
        if n > 0 && by_hash[n - 1].0 != h {
            run_start = out.len();
        }
        let (c, k) = &pairs[i as usize];
        // (the groups of this hash: one, unless two items collide)
        match out[run_start..].iter_mut().find(|(x, _)| x == c) {
            Some((_, m)) => {
                if m.0.last().is_none_or(|l| l.0 != *k) {
                    m.0.push((*k, ()));
                }
            }
            None => out.push((c.clone(), KeyMap(alloc::vec![(*k, ())]))),
        }
    }
    // (the numbered ones are in order among themselves already)
    out[numbered..].sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let (num, rest) = out.split_at(numbered);
    debug_assert!(num.is_sorted_by(|a, b| a.0 < b.0));
    let _ = (num, rest);
    let mut merged: Vec<(K, KeyMap<()>)> = Vec::with_capacity(out.len());
    let mut rest = out.split_off(numbered).into_iter().peekable();
    for g in out {
        while rest.peek().is_some_and(|r| r.0 < g.0) {
            merged.extend(rest.next());
        }
        merged.push(g);
    }
    merged.extend(rest);
    merged.into_iter().collect()
}

/// A fast hash for grouping (not stable across builds: never stored).
struct Fx(u64);

impl core::hash::Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for c in bytes.chunks(8) {
            let mut w = [0u8; 8];
            w[..c.len()].copy_from_slice(c);
            self.write_u64(u64::from_le_bytes(w));
        }
    }
    fn write_u64(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_u32(&mut self, x: u32) {
        self.write_u64(u64::from(x));
    }
    fn write_u8(&mut self, x: u8) {
        self.write_u64(u64::from(x));
    }
    fn write_usize(&mut self, x: usize) {
        self.write_u64(x as u64);
    }
    fn write_i32(&mut self, x: i32) {
        self.write_u64(u64::from(x.cast_unsigned()));
    }
    fn write_isize(&mut self, x: isize) {
        self.write_u64(x.cast_unsigned() as u64);
    }
}

impl<V> Default for KeyMap<V> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<V: Copy> KeyMap<V> {
    fn insert(&mut self, k: u64, v: V) {
        if self.0.last().is_none_or(|l| l.0 < k) {
            // (keys come in order when indexing a run: appended)
            if self.0.len() == self.0.capacity() {
                self.0.reserve_exact(1 + self.0.len() / 4);
            }
            self.0.push((k, v));
            return;
        }
        match self.0.binary_search_by_key(&k, |e| e.0) {
            Ok(i) => self.0[i].1 = v,
            Err(i) => {
                // (most entries stay at one or two keys: grow by a quarter)
                if self.0.len() == self.0.capacity() {
                    self.0.reserve_exact(1 + self.0.len() / 4);
                }
                self.0.insert(i, (k, v));
            }
        }
    }

    fn remove(&mut self, k: u64) {
        if let Ok(i) = self.0.binary_search_by_key(&k, |e| e.0) {
            self.0.remove(i);
            if self.0.capacity() > 2 * self.0.len() + 4 {
                self.0.shrink_to_fit();
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn iter(&self) -> core::slice::Iter<'_, (u64, V)> {
        self.0.iter()
    }

    fn range(&self, r: impl core::ops::RangeBounds<u64>) -> core::slice::Iter<'_, (u64, V)> {
        use core::ops::Bound;
        let lo = match r.start_bound() {
            Bound::Included(&a) => self.0.partition_point(|e| e.0 < a),
            Bound::Excluded(&a) => self.0.partition_point(|e| e.0 <= a),
            Bound::Unbounded => 0,
        };
        let hi = match r.end_bound() {
            Bound::Included(&b) => self.0.partition_point(|e| e.0 <= b),
            Bound::Excluded(&b) => self.0.partition_point(|e| e.0 < b),
            Bound::Unbounded => self.0.len(),
        };
        self.0[lo..hi.max(lo)].iter()
    }
}

#[cfg(test)]
mod tests {
    use super::CellIndex;
    use alloc::collections::{BTreeMap, BTreeSet};
    use alloc::vec::Vec;

    /// Random adds and removals of keys for cells numbered densely (a
    /// third of them, sparsely across pages) or not (by hash, through the
    /// table's growth), and clears: the same keys as a B-tree's.
    #[test]
    fn cell_index_matches_a_b_tree() {
        let number = |c: &u64| {
            c.is_multiple_of(3)
                .then(|| u32::try_from(c / 3 * 997).unwrap_or(0))
        };
        let mut ix: CellIndex<u64> = CellIndex::new(number);
        let mut want: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for step in 0..200_000u32 {
            let (c, k) = (next() % 5000, next() % 64);
            match next() % 10 {
                0..=5 => {
                    ix.add(&c, k);
                    want.entry(c).or_default().insert(k);
                }
                6..=8 => {
                    ix.remove_key(&c, k);
                    if let Some(s) = want.get_mut(&c) {
                        s.remove(&k);
                    }
                }
                _ if step % 50_000 == 7 => {
                    ix.clear();
                    want.clear();
                }
                _ if step % 101 == 3 => {
                    // (a splice: keys removed, then others added, staged)
                    ix.begin();
                    for _ in 0..next() % 300 {
                        let c = next() % 5000;
                        if let Some(&k) = want.get(&c).and_then(|s| s.iter().next()) {
                            ix.remove_key(&c, k);
                            want.get_mut(&c).map(|s| s.remove(&k));
                        }
                    }
                    for _ in 0..next() % 300 {
                        let (c, k) = (next() % 5000, 64 + next() % 64);
                        ix.add(&c, k);
                        want.entry(c).or_default().insert(k);
                    }
                    if next() % 4 == 0 {
                        ix.clear();
                        want.clear();
                        ix.add(&1, 5);
                        want.entry(1).or_default().insert(5);
                    }
                    ix.flush();
                }
                _ => {}
            }
            if step.is_multiple_of(997) {
                for c in 0..5000 {
                    let got: Vec<u64> = ix
                        .get(&c)
                        .map(|m| m.iter().map(|e| e.0).collect())
                        .unwrap_or_default();
                    let w: Vec<u64> = want
                        .get(&c)
                        .map(|s| s.iter().copied().collect())
                        .unwrap_or_default();
                    assert_eq!(got, w, "cell {c} at step {step}");
                }
            }
        }
    }
}
