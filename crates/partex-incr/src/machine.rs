//! The interface a language implements (`DESIGN.md` §7.0).

use alloc::vec::Vec;
use core::fmt::Debug;
use core::hash::Hash;

use crate::hash::Version;
use crate::link::LinkCtx;

/// The candidate level of a *layer* edge ([`Step::Candidate`]): a region
/// is always cut there, whatever its cost (TeX: just before and just
/// after a page's shipout, which is then a region of its own).
pub const LAYER: u8 = 4;

/// What one [`Machine::step`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// Ran; the machine is not at a point where a region may end.
    Continue,
    /// Ran; the machine is now at a candidate boundary of the given
    /// level (0 finest; higher levels are coarser, preferred cuts: a
    /// blank line, a section, a file edge; [`LAYER`] and above: always a
    /// cut).
    Candidate(u8),
    /// The program has ended.
    Halt,
    /// A forced hole under the suspend policy: the step did nothing (no
    /// write, no effect, no position change) and runs again once the hole
    /// is resolved.
    Suspended,
}

/// A symbolic value: "the entry value of one cell of the region that
/// planted it". Hole numbers are local to a region trace.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Hole(pub u32);

/// An integer that stays symbolic under addition: `hole + k` (§7.5).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Affine {
    pub hole: Hole,
    pub k: i64,
}

impl Affine {
    #[must_use]
    pub fn plus(self, d: i64) -> Self {
        Self {
            hole: self.hole,
            k: self.k.wrapping_add(d),
        }
    }
}

/// A relocation of the numbers one origin cell gives out (`DESIGN.md`
/// 4.1, relocatable values): those above `base` moved by `delta`, which
/// is positive (so that the relocation keeps numbers apart and in order).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Shift<C> {
    pub cell: C,
    pub base: i64,
    pub delta: i64,
}

impl<C> Shift<C> {
    /// Number `x`, relocated.
    #[must_use]
    pub fn map(&self, x: i64) -> i64 {
        if x > self.base { x + self.delta } else { x }
    }
}

/// The answer to [`Recorder::force`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Forced<V> {
    /// Speculate: go on with this value (the runtime has recorded a guard
    /// on it).
    Value(V),
    /// Suspend: return [`Step::Suspended`] from the current step.
    Suspend,
}

/// A deterministic interpreter over cells.
///
/// Contract, checked by the tests and by `build::Config::sanitize`:
/// - `step` is a function of the cells it reports through
///   [`Recorder::read`] (and of the position): every observation of
///   state is reported before it is used, every mutation through
///   [`Recorder::write`], every output through [`Recorder::effect`];
/// - `get` and `set` expose exactly those cells, and `get` is canonical
///   (equal states give equal values);
/// - the position ([`at`](Machine::at)) is not a cell: replay moves it
///   with [`seek`](Machine::seek).
pub trait Machine: Clone + Send + Sync {
    /// A unit of state.
    type Cell: Clone + Ord + Hash + Debug + Send + Sync;
    /// A cell's content; its version is its stable hash.
    type Value: Clone + Hash + Eq + Debug + Send + Sync;
    /// What leaves the machine, as a value (equal effects link to equal
    /// output).
    type Effect: Clone + PartialEq + Debug + Send + Sync;
    /// Where a region may begin or end: the machine's position.
    type Boundary: Clone + Ord + Hash + Debug + Send + Sync;

    /// Run one step.
    fn step<R: Recorder<Self>>(&mut self, r: &mut R) -> Step;
    /// A cell's value (`None`: absent), for validation.
    fn get(&self, c: &Self::Cell) -> Option<Self::Value>;
    /// Replace a cell's value (`None`: remove it), for replay and patching.
    fn set(&mut self, c: &Self::Cell, v: Option<Self::Value>);
    /// Set many cells at once, `writes` sorted by cell: what setting each
    /// in order does. A machine may do it faster (TeX: one snapshot taken
    /// with the words it already holds).
    fn set_all(&mut self, writes: &[(&Self::Cell, &Self::Value)]) {
        for (c, v) in writes {
            self.set(c, Some((*v).clone()));
        }
    }
    /// The current position.
    fn at(&self) -> Self::Boundary;
    /// Move to a position: the exit of a replayed region.
    fn seek(&mut self, b: &Self::Boundary);
    /// The whole-state hash (§5.3), for the sanitizer and the tests.
    fn digest(&self) -> Version;
    /// A small number for `c`, if it is one of many cells numbered
    /// densely from 0 (a register): building the indexes of a loaded build
    /// groups those by number rather than by comparing cells. Equal
    /// numbers must mean equal cells, and a numbered cell must sort
    /// against the others as its number does against theirs.
    fn dense_cell(_c: &Self::Cell) -> Option<u32> {
        None
    }

    /// Whether `b` is a [`LAYER`] edge (a region is always cut there):
    /// [`crate::Build::coarsen`] merges no regions across one. By default
    /// none is.
    fn layer_edge(_b: &Self::Boundary) -> bool {
        false
    }

    /// A region ends here: the runtime is about to read the cells the
    /// region wrote ([`Machine::get`]). A machine may report reads and
    /// writes it kept back until now (they count as the region's, read
    /// before written as the machine says), and prepare values that are
    /// costly to compute from `&self` (TeX: a snapshot and the memoized
    /// state hash). By default nothing.
    fn prepare_cut<R: Recorder<Self>>(&mut self, _r: &mut R) {}

    /// Replay the clean regions `span` at once: become the old run's state
    /// at the end of the last one (its writes hold what the machine needs
    /// to find it, TeX: a snapshot), patched with `d`, the cells whose
    /// values differ from the old run's there; accumulating cells keep
    /// their value here plus every write of the span. False if the
    /// machine cannot (the runtime applies the writes instead).
    fn replay_exit(
        &mut self,
        _span: &[&crate::trace::Trace<Self>],
        _d: &mut dyn Iterator<Item = (&Self::Cell, &Option<Self::Value>)>,
    ) -> bool {
        false
    }

    /// Render one effect at the link step. Holes and symbols resolve
    /// through `cx`.
    fn render(e: &Self::Effect, cx: &mut LinkCtx<'_, Self>, out: &mut Vec<u8>);
    /// How many link-time symbols (§7.6 "numbering is symbolic") an effect
    /// allocates; `render` takes them in order with [`LinkCtx::alloc`].
    fn allocs(_e: &Self::Effect) -> u64 {
        0
    }

    /// Whether setting `c` adds to its value instead of replacing it (an
    /// accumulator: TeX's glyphs used). Replaying several regions at once
    /// then applies every write of such a cell, in order, not only the
    /// last.
    fn accumulates(_c: &Self::Cell) -> bool {
        false
    }

    /// The value of accumulating cell `c` that adds `a`, then `b` (composing
    /// two regions' writes). By default `b`.
    fn combine(_c: &Self::Cell, _a: &Self::Value, b: &Self::Value) -> Self::Value {
        b.clone()
    }

    /// The accumulating cell that `c` asks a question about, if `c` is a
    /// *derived* cell: [`Machine::get`] answers it from the state (TeX:
    /// the final number of one object, a question about the numbering);
    /// no region writes it and setting it does nothing.
    ///
    /// A region that asks such questions guards its answers and keeps
    /// its guard on the source as well (which is how a rebuild finds it
    /// when the source differs). When the source is the only thing that
    /// made it dirty, the rebuild checks the answers at the region's
    /// entry instead of the source's version: a derived guard can make
    /// a region cleaner than its source guard would, never dirtier. A
    /// region that reads the source whole has no derived guards on it,
    /// and its source guard is checked as for any accumulating cell.
    ///
    /// **A requirement on implementors:** a region that read the source
    /// whole (any read other than through its derived cells) must record
    /// no derived guard on it, only its guard on the source. The runtime
    /// takes derived guards on a source to mean that the region read it
    /// only through them ([`crate::Trace::answered`]).
    fn derived(_c: &Self::Cell) -> Option<Self::Cell> {
        None
    }

    /// The output of a run as the sanitizer compares two runs'
    /// (`build::Config::sanitize`): by default as it is. A machine leaves
    /// out what it does not reproduce on purpose, which may differ between
    /// a rebuild and a fresh run of the same input (TeX: the statistics of
    /// its internal tables in the log).
    #[must_use]
    fn comparable_output(out: Vec<u8>) -> Vec<u8> {
        out
    }

    /// A small dense number for cell `c`, if it has one: flat recording
    /// (`build::Config::flat`) keeps such cells in bit sets instead of
    /// ordered maps. Cells without one are recorded in maps.
    fn index(_c: &Self::Cell) -> Option<u32> {
        None
    }

    /// Relocatable values (`DESIGN.md` 4.1): if `c` is an *origin* cell,
    /// a counter whose numbers the program may only copy, compare and
    /// shift, the number it holds in `v`. A rebuild that finds an origin
    /// differing by a positive amount after a re-run span relocates the
    /// regions after it that differ only by the shift ([`Shift`]). By
    /// default none is.
    fn origin_number(_c: &Self::Cell, _v: Option<&Self::Value>) -> Option<i64> {
        None
    }

    /// Whether guard `c` (a guard on the numbers themselves, which always
    /// holds as such: an origin used as a number, an answer about one)
    /// still holds for a region relocated by `shifts`. By default yes.
    fn guard_relocates(_c: &Self::Cell, _shifts: &[Shift<Self::Cell>]) -> bool {
        true
    }

    /// Guard `c` of a region relocated by `shifts`, as the relocated
    /// region would have recorded it (an answer about a number names the
    /// number). By default `c`.
    fn relocate_guard(c: &Self::Cell, _shifts: &[Shift<Self::Cell>]) -> Self::Cell {
        c.clone()
    }

    /// Derived guard `c` ([`Machine::derived`]) holding `v`, an answer
    /// about the numbers themselves, as a run whose numbers moved by
    /// `shifts` would have recorded it (the question renamed, the answer
    /// relocated), if `self`, the state at the region's entry, answers
    /// so: the guard as it holds there. `None` if it does not or cannot
    /// be told (the region then runs). By default `None`.
    fn relocate_answer(
        &mut self,
        _c: &Self::Cell,
        _v: Version,
        _shifts: &[Shift<Self::Cell>],
    ) -> Option<(Self::Cell, Version)> {
        None
    }

    /// Value `v` of cell `c` with every number of a moving origin
    /// relocated by `shifts`: `Ok(None)` if it holds none, `Err` if the
    /// machine cannot tell or relocate it (the region then runs). By
    /// default `Err`: no relocation.
    #[allow(clippy::result_unit_err)]
    fn relocate_value(
        &mut self,
        _c: &Self::Cell,
        _v: &Self::Value,
        _shifts: &[Shift<Self::Cell>],
    ) -> Result<Option<Self::Value>, ()> {
        Err(())
    }

    /// The value that makes cell `c` hold hole `h`, if `c` can hold one.
    fn hole_value(_c: &Self::Cell, _h: Hole) -> Option<Self::Value> {
        None
    }
    /// `v` with every resolvable hole replaced.
    fn subst_value(v: &Self::Value, _env: &dyn Fn(Hole) -> Option<Self::Value>) -> Self::Value {
        v.clone()
    }
    /// `e` with every resolvable hole replaced.
    fn subst_effect(e: &Self::Effect, _env: &dyn Fn(Hole) -> Option<Self::Value>) -> Self::Effect {
        e.clone()
    }
    /// Replace every resolvable hole in the state: resuming a suspended
    /// region.
    fn subst_state(&mut self, _env: &dyn Fn(Hole) -> Option<Self::Value>) {}
}

/// The tracker, seen from the machine.
pub trait Recorder<M: Machine> {
    /// The machine observed `c`, which held `v`.
    fn read(&mut self, c: &M::Cell, v: Option<&M::Value>);
    /// The machine set `c` to `v`.
    fn write(&mut self, c: &M::Cell, v: &M::Value);
    /// The machine emitted `e`.
    fn effect(&mut self, e: M::Effect);
    /// The machine needs the concrete value of hole `h` (a forcing point).
    fn force(&mut self, h: Hole) -> Forced<M::Value>;
    /// Work done, in the machine's own units; the chooser sizes regions by
    /// it. Each step counts one unit anyway.
    fn cost(&mut self, _units: u64) {}
    /// Whether [`Recorder::write`] keeps the values written (a machine
    /// may then skip computing them). Flat recording reads them from the
    /// state at the region's end instead.
    fn wants_values(&self) -> bool {
        true
    }
}

/// Cold-start splitting (§7.7): positions where regions may start, found
/// without running the program (TeX: top-level blank lines, `\input`
/// edges).
pub trait Split: Machine {
    /// About `parts` positions in program order, the first being
    /// [`Machine::at`].
    fn split(&self, parts: usize) -> Vec<Self::Boundary>;

    /// Whether `b`, a boundary of a previous run, is still a position of
    /// this program (a warm start keeps those; §7.7). Boundaries kept must
    /// stay in program order. By default none is.
    fn still_at(&self, _b: &Self::Boundary) -> bool {
        false
    }
}
