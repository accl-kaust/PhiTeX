//! Region traces (`DESIGN.md` §7.3): guards plus effects.
//!
//! A trace is what one region did, recorded while it ran: the cells it
//! read before writing them, with their versions (the guards), its net
//! writes, the effects it emitted, and the holes it was started with. It
//! is replayed wherever its guards hold, and composes with its neighbour
//! into the trace of a larger region.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::hash::{Version, version_of};
use crate::machine::{Forced, Hole, Machine, Recorder};

/// What one region did.
#[derive(Clone, Debug)]
pub struct Trace<M: Machine> {
    /// Where the region began.
    pub entry: M::Boundary,
    /// Where it ended (its successor's entry).
    pub exit: M::Boundary,
    /// First reads with their versions, sorted by cell. A cell planted
    /// with a hole is not guarded; forcing it under the speculate policy
    /// guards the value assumed.
    pub guards: Vec<(M::Cell, Version)>,
    /// Net writes with their versions, sorted by cell. Values may hold
    /// this trace's holes.
    pub writes: Vec<(M::Cell, M::Value, Version)>,
    /// Emitted output, in order. May hold this trace's holes.
    pub effects: Vec<M::Effect>,
    /// Hole `i` is the entry value of cell `holes[i]`.
    pub holes: Vec<M::Cell>,
    /// Link-time symbols the effects allocate.
    pub allocs: u64,
    /// Steps plus the machine's own cost units.
    pub cost: u64,
    /// The rebuild that recorded it (0: the first build): what the
    /// machine keeps of its exit state for [`crate::Machine::replay_exit`]
    /// holds that run's values of the cells no region wrote.
    pub born: u32,
    /// The last rebuild that relocated it (`Machine::relocate_value`; 0:
    /// none): the exit state kept holds its writes as they were before.
    pub moved: u32,
}

impl<M: Machine> Trace<M> {
    /// Whether every guard holds in `m` (the region's entry state).
    pub fn holds(&self, m: &M) -> bool {
        self.guards.iter().all(|(c, v)| version_of(&m.get(c)) == *v)
    }

    /// Whether the region read `c` before writing it.
    pub fn reads(&self, c: &M::Cell) -> bool {
        self.guards.binary_search_by(|(g, _)| g.cmp(c)).is_ok()
    }

    /// The guards on cells derived from `src` ([`Machine::derived`]): the
    /// questions the region asked about it.
    pub fn derived_guards<'a>(
        &'a self,
        src: &'a M::Cell,
    ) -> impl Iterator<Item = &'a (M::Cell, Version)> + 'a {
        self.guards
            .iter()
            .filter(move |(g, _)| M::derived(g).as_ref() == Some(src))
    }

    /// Whether the region's reading of `src` is only questions: it has
    /// derived guards on it (and then a guard on `src` too, which finds
    /// it; [`Machine::derived`]).
    pub fn answered(&self, src: &M::Cell) -> bool {
        self.derived_guards(src).next().is_some()
    }

    /// The values of this trace's holes in its entry state `m`.
    ///
    /// # Panics
    ///
    /// If a hole's cell is absent in `m` (holes are only planted in cells
    /// that hold a value).
    pub fn env(&self, m: &M) -> Vec<M::Value> {
        self.holes
            .iter()
            .map(|c| m.get(c).expect("a hole's cell holds a value"))
            .collect()
    }

    /// Replay: apply the writes (holes resolved by `env`) and move to the
    /// exit.
    pub fn apply(&self, m: &mut M, env: &[M::Value]) {
        if env.is_empty() {
            for (c, v, _) in &self.writes {
                m.set(c, Some(v.clone()));
            }
        } else {
            let look = |h: Hole| env.get(h.0 as usize).cloned();
            for (c, v, _) in &self.writes {
                m.set(c, Some(M::subst_value(v, &look)));
            }
        }
        m.seek(&self.exit);
    }

    /// The trace of this region followed by `next` (both without holes):
    /// writes in order, guards the union less what this one wrote, effects
    /// concatenated, costs summed. An accumulating cell's writes combine
    /// ([`Machine::combine`]); `next` reading one this region added to
    /// depends on its value at this region's entry, `entry(c)` (the
    /// composed trace gets that guard; `None`: unknown, so it cannot be
    /// composed and `None` is returned).
    #[must_use]
    pub fn compose_with(
        &self,
        next: &Self,
        entry: &dyn Fn(&M::Cell) -> Option<Version>,
    ) -> Option<Self> {
        let mut c = Composer::new(self);
        c.push(next, entry).then(|| c.finish())
    }

    /// [`Trace::compose_with`] for machines without accumulating cells.
    ///
    /// # Panics
    ///
    /// If `next` reads an accumulating cell this region wrote.
    #[must_use]
    pub fn compose(&self, next: &Self) -> Self {
        self.compose_with(next, &|_| None)
            .expect("no accumulating cell read after it was added to")
    }
}

/// A trace composed region by region ([`Trace::compose_with`] of each
/// next region in turn), its guards and writes kept as maps from one
/// region to the next rather than made again from vectors at each (a
/// run of regions merged costs the regions, not their number times the
/// run).
pub struct Composer<M: Machine> {
    first: Trace<M>,
    guards: BTreeMap<M::Cell, Version>,
    writes: BTreeMap<M::Cell, (M::Value, Version)>,
    /// The sources the guards ask about ([`Machine::derived`] of each).
    asked: BTreeSet<M::Cell>,
}

impl<M: Machine> Composer<M> {
    /// The composition of `first` alone.
    #[must_use]
    pub fn new(first: &Trace<M>) -> Self {
        let mut t = first.clone();
        let guards: BTreeMap<M::Cell, Version> =
            core::mem::take(&mut t.guards).into_iter().collect();
        let writes = core::mem::take(&mut t.writes)
            .into_iter()
            .map(|(c, v, h)| (c, (v, h)))
            .collect();
        let asked = guards.keys().filter_map(M::derived).collect();
        Self {
            first: t,
            guards,
            writes,
            asked,
        }
    }

    /// The cost so far.
    #[must_use]
    pub fn cost(&self) -> u64 {
        self.first.cost
    }

    /// Compose `next` after what is composed so far, as
    /// [`Trace::compose_with`] does: `false` (and nothing changed) if it
    /// cannot be.
    pub fn push(&mut self, next: &Trace<M>, entry: &dyn Fn(&M::Cell) -> Option<Version>) -> bool {
        debug_assert!(self.first.holes.is_empty() && next.holes.is_empty());
        let wrote = |c: &M::Cell| self.writes.contains_key(c);
        // (the guards `next` adds, each cell once in it: decided against
        // the guards before it)
        let mut added: Vec<(M::Cell, Version)> = Vec::new();
        for (c, v) in &next.guards {
            if M::derived(c).is_some_and(|src| wrote(&src)) {
                // (a question about a cell this region added to: its
                // answer at the composed entry is unknown, so the
                // composed region reads the source whole, below)
                continue;
            }
            if !wrote(c) {
                if !self.guards.contains_key(c) {
                    added.push((c.clone(), *v));
                }
            } else if M::accumulates(c) && !self.guards.contains_key(c) {
                let Some(e) = entry(c) else {
                    return false;
                };
                added.push((c.clone(), e));
            }
        }
        // The composed region reads a source only through questions if
        // each part that reads it does, and the second asks none about
        // what the first added to: otherwise it reads it whole.
        let theirs: BTreeSet<M::Cell> = next
            .guards
            .iter()
            .filter_map(|(c, _)| M::derived(c))
            .collect();
        let whole: BTreeSet<M::Cell> = self
            .asked
            .union(&theirs)
            .filter(|src| {
                let mine = !self.guards.contains_key(*src) || self.asked.contains(*src);
                let next_part = !next.reads(src) || theirs.contains(*src);
                let asked_after = wrote(src) && next.reads(src);
                !(mine && next_part) || asked_after
            })
            .cloned()
            .collect();
        for (c, v) in added {
            if let Some(src) = M::derived(&c) {
                self.asked.insert(src);
            }
            self.guards.insert(c, v);
        }
        if !whole.is_empty() {
            self.guards
                .retain(|c, _| M::derived(c).as_ref().is_none_or(|s| !whole.contains(s)));
            self.asked.retain(|s| !whole.contains(s));
        }
        for (c, v, h) in &next.writes {
            let v = match self.writes.get(c) {
                Some((a, _)) if M::accumulates(c) => M::combine(c, a, v),
                _ => v.clone(),
            };
            self.writes.insert(c.clone(), (v, *h));
        }
        let t = &mut self.first;
        t.exit = next.exit.clone();
        t.effects.extend(next.effects.iter().cloned());
        t.allocs += next.allocs;
        t.cost += next.cost;
        t.born = t.born.min(next.born);
        t.moved = t.moved.max(next.moved);
        true
    }

    /// The trace composed.
    #[must_use]
    pub fn finish(self) -> Trace<M> {
        Trace {
            guards: self.guards.into_iter().collect(),
            writes: self
                .writes
                .into_iter()
                .map(|(c, (v, h))| (c, v, h))
                .collect(),
            ..self.first
        }
    }
}

/// How a forced hole is answered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnForce {
    Suspend,
    Speculate,
}

/// The recording tracker: builds a [`Trace`] while the machine runs.
#[derive(Clone, Debug)]
pub(crate) struct Recording<M: Machine> {
    entry: M::Boundary,
    guards: BTreeMap<M::Cell, Version>,
    writes: BTreeMap<M::Cell, M::Value>,
    effects: Vec<M::Effect>,
    /// Planted holes: cell to hole, and each hole's cell with the guessed
    /// entry value speculation assumes.
    hole_cells: BTreeMap<M::Cell, Hole>,
    holes: Vec<(M::Cell, M::Value)>,
    pub(crate) on_force: OnForce,
    pub(crate) allocs: u64,
    pub(crate) cost: u64,
}

impl<M: Machine> Recording<M> {
    pub(crate) fn new(entry: M::Boundary, on_force: OnForce) -> Self {
        Self {
            entry,
            guards: BTreeMap::new(),
            writes: BTreeMap::new(),
            effects: Vec::new(),
            hole_cells: BTreeMap::new(),
            holes: Vec::new(),
            on_force,
            allocs: 0,
            cost: 0,
        }
    }

    /// Plant a hole in `c` of `m`, which holds `guess` (the entry value
    /// speculation assumes). Returns false if `c` cannot hold a hole.
    pub(crate) fn plant(&mut self, m: &mut M, c: &M::Cell, guess: M::Value) -> bool {
        let h = Hole(u32::try_from(self.holes.len()).expect("fewer than 2^32 holes"));
        let Some(v) = M::hole_value(c, h) else {
            return false;
        };
        m.set(c, Some(v));
        self.hole_cells.insert(c.clone(), h);
        self.holes.push((c.clone(), guess));
        true
    }

    /// Whether every guard recorded so far holds in `m`.
    pub(crate) fn holds(&self, m: &M) -> bool {
        self.guards.iter().all(|(c, v)| version_of(&m.get(c)) == *v)
    }

    /// The writes recorded so far, as a guess for the successor.
    pub(crate) fn writes(&self) -> impl Iterator<Item = (&M::Cell, &M::Value)> {
        self.writes.iter()
    }

    pub(crate) fn hole_cells(&self) -> impl Iterator<Item = &M::Cell> {
        self.holes.iter().map(|(c, _)| c)
    }

    /// Resolve every hole with its value in the (now known) entry state
    /// `entry`: guard those values, substitute them in the writes and
    /// effects recorded so far, and in the suspended machine `m`. False,
    /// changing nothing, if a hole's cell is absent from `entry`.
    pub(crate) fn resolve(&mut self, entry: &M, m: &mut M) -> bool {
        if self.holes.is_empty() {
            return true;
        }
        let Some(env) = self
            .holes
            .iter()
            .map(|(c, _)| entry.get(c))
            .collect::<Option<Vec<M::Value>>>()
        else {
            return false;
        };
        let look = |h: Hole| env.get(h.0 as usize).cloned();
        for (c, _) in &self.holes {
            self.guards.insert(c.clone(), version_of(&entry.get(c)));
        }
        for v in self.writes.values_mut() {
            *v = M::subst_value(v, &look);
        }
        for e in &mut self.effects {
            *e = M::subst_effect(e, &look);
        }
        m.subst_state(&look);
        self.holes.clear();
        self.hole_cells.clear();
        true
    }

    pub(crate) fn finish(self, exit: M::Boundary) -> Trace<M> {
        Trace {
            entry: self.entry,
            exit,
            guards: self.guards.into_iter().collect(),
            writes: self
                .writes
                .into_iter()
                .map(|(c, v)| {
                    let h = version_of(&Some(&v));
                    (c, v, h)
                })
                .collect(),
            effects: self.effects,
            holes: self.holes.into_iter().map(|(c, _)| c).collect(),
            allocs: self.allocs,
            cost: self.cost,
            born: 0,
            moved: 0,
        }
    }
}

impl<M: Machine> Recorder<M> for Recording<M> {
    fn read(&mut self, c: &M::Cell, v: Option<&M::Value>) {
        if self.writes.contains_key(c) || self.hole_cells.contains_key(c) {
            return;
        }
        if !self.guards.contains_key(c) {
            self.guards.insert(c.clone(), version_of(&v));
        }
    }

    fn write(&mut self, c: &M::Cell, v: &M::Value) {
        self.writes.insert(c.clone(), v.clone());
    }

    fn effect(&mut self, e: M::Effect) {
        self.allocs += M::allocs(&e);
        self.effects.push(e);
    }

    fn force(&mut self, h: Hole) -> Forced<M::Value> {
        let Some((c, guess)) = self.holes.get(h.0 as usize) else {
            return Forced::Suspend;
        };
        match self.on_force {
            OnForce::Suspend => Forced::Suspend,
            OnForce::Speculate => {
                // A guard on the entry value, whatever the region wrote to
                // the cell since.
                self.guards.insert(c.clone(), version_of(&Some(guess)));
                Forced::Value(guess.clone())
            }
        }
    }

    fn cost(&mut self, units: u64) {
        self.cost += units;
    }
}

/// What a region recorder does for the region loop of a build.
pub(crate) trait RegionRecorder<M: Machine>: Recorder<M> {
    /// Units of work so far in this region.
    fn cost_mut(&mut self) -> &mut u64;
    /// End the region at `m`'s position and start the next one there.
    fn cut(&mut self, m: &M) -> Trace<M>;
}

impl<M: Machine> RegionRecorder<M> for Recording<M> {
    fn cost_mut(&mut self) -> &mut u64 {
        &mut self.cost
    }

    fn cut(&mut self, m: &M) -> Trace<M> {
        let at = m.at();
        let next = Recording::new(at.clone(), self.on_force);
        core::mem::replace(self, next).finish(at)
    }
}

/// Flat recording (`DESIGN.md` §7.0, "TeX's adapter"): the first touch of
/// a cell with a dense number ([`Machine::index`]) is found in a bit set,
/// not an ordered map; a first read keeps its version, a first write only
/// the cell (the values written are taken from the state at the region's
/// end). The bits are cleared per region by what was touched, so a region
/// costs what it touched. No holes: builds only.
pub(crate) struct FlatRecording<M: Machine> {
    entry: M::Boundary,
    /// Touched (read or written) and written, by dense number.
    touched: Vec<u64>,
    written: Vec<u64>,
    guards: Vec<(M::Cell, Version)>,
    wrote: Vec<M::Cell>,
    /// Cells without a dense number.
    far_touched: BTreeSet<M::Cell>,
    far_written: BTreeSet<M::Cell>,
    effects: Vec<M::Effect>,
    allocs: u64,
    cost: u64,
}

fn bit(v: &mut Vec<u64>, i: u32) -> bool {
    let (w, b) = ((i >> 6) as usize, 1u64 << (i & 63));
    if w >= v.len() {
        v.resize(w + 1 + w / 2, 0);
    }
    let old = v[w] & b != 0;
    v[w] |= b;
    old
}

fn clear_bit(v: &mut [u64], i: u32) {
    if let Some(w) = v.get_mut((i >> 6) as usize) {
        *w &= !(1u64 << (i & 63));
    }
}

impl<M: Machine> FlatRecording<M> {
    pub(crate) fn new(entry: M::Boundary) -> Self {
        Self {
            entry,
            touched: Vec::new(),
            written: Vec::new(),
            guards: Vec::new(),
            wrote: Vec::new(),
            far_touched: BTreeSet::new(),
            far_written: BTreeSet::new(),
            effects: Vec::new(),
            allocs: 0,
            cost: 0,
        }
    }
}

impl<M: Machine> Recorder<M> for FlatRecording<M> {
    #[inline]
    fn read(&mut self, c: &M::Cell, v: Option<&M::Value>) {
        let first = match M::index(c) {
            Some(i) => !bit(&mut self.touched, i),
            None => self.far_touched.insert(c.clone()),
        };
        if first {
            self.guards.push((c.clone(), version_of(&v)));
        }
    }

    #[inline]
    fn write(&mut self, c: &M::Cell, _v: &M::Value) {
        let first = if let Some(i) = M::index(c) {
            bit(&mut self.touched, i);
            !bit(&mut self.written, i)
        } else {
            self.far_touched.insert(c.clone());
            self.far_written.insert(c.clone())
        };
        if first {
            self.wrote.push(c.clone());
        }
    }

    fn effect(&mut self, e: M::Effect) {
        self.allocs += M::allocs(&e);
        self.effects.push(e);
    }

    fn force(&mut self, _h: Hole) -> Forced<M::Value> {
        Forced::Suspend
    }

    fn cost(&mut self, units: u64) {
        self.cost += units;
    }

    fn wants_values(&self) -> bool {
        false
    }
}

impl<M: Machine> RegionRecorder<M> for FlatRecording<M> {
    fn cost_mut(&mut self) -> &mut u64 {
        &mut self.cost
    }

    fn cut(&mut self, m: &M) -> Trace<M> {
        for (c, _) in &self.guards {
            if let Some(i) = M::index(c) {
                clear_bit(&mut self.touched, i);
            }
        }
        for c in &self.wrote {
            if let Some(i) = M::index(c) {
                clear_bit(&mut self.touched, i);
                clear_bit(&mut self.written, i);
            }
        }
        self.far_touched.clear();
        self.far_written.clear();
        let mut guards = core::mem::take(&mut self.guards);
        guards.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        // (kept for the build's life: no room to grow)
        guards.shrink_to_fit();
        let mut writes: Vec<(M::Cell, M::Value, Version)> = core::mem::take(&mut self.wrote)
            .into_iter()
            .map(|c| {
                let v = m.get(&c).expect("a cell written holds a value");
                let h = version_of(&Some(&v));
                (c, v, h)
            })
            .collect();
        writes.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let mut effects = core::mem::take(&mut self.effects);
        effects.shrink_to_fit();
        let at = m.at();
        Trace {
            entry: core::mem::replace(&mut self.entry, at.clone()),
            exit: at,
            guards,
            writes,
            effects,
            holes: Vec::new(),
            allocs: core::mem::take(&mut self.allocs),
            cost: core::mem::take(&mut self.cost),
            born: 0,
            moved: 0,
        }
    }
}

/// The tracker of a plain run: effects only.
pub(crate) struct Plain<M: Machine> {
    pub(crate) effects: Vec<M::Effect>,
}

impl<M: Machine> Recorder<M> for Plain<M> {
    #[inline]
    fn read(&mut self, _c: &M::Cell, _v: Option<&M::Value>) {}
    #[inline]
    fn write(&mut self, _c: &M::Cell, _v: &M::Value) {}
    #[inline]
    fn effect(&mut self, e: M::Effect) {
        self.effects.push(e);
    }
    fn force(&mut self, _h: Hole) -> Forced<M::Value> {
        Forced::Suspend
    }
}
