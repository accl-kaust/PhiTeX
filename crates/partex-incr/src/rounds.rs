//! Parallel rounds with holes (`DESIGN.md` §7.5, §7.7).
//!
//! A run is split into regions at positions found without running
//! ([`Split`]). Each round:
//!
//! 1. **Sweep** (sequential, program order): walk the regions keeping a
//!    composed entry state. A region whose latest trace starts at the
//!    right position and whose guards hold there is valid: its writes are
//!    applied, holes resolved by the entry, whether or not its
//!    predecessors reran. While every region so far is valid the entry is
//!    *exact*; a suspended region met there is resumed at once with its
//!    holes resolved. An invalid region is queued with the composed entry
//!    as its guess, and its old writes still compose the next guess.
//! 2. **Run** (parallel): every queued region runs from its guess. Cells
//!    that more than one region wrote in the latest traces are planted as
//!    holes (the volatility predictor, learned from the previous round);
//!    the first invalid region, whose entry is exact, gets none.
//!
//! **Warm starts** ([`run_warm`]): a previous run's regions, as a
//! [`Warm`] taken from its [`Outcome`] or from a recorded [`Build`], seed
//! the slots before round 1. The first sweep then validates them against
//! the composed entries like any other trace, so an unchanged region costs
//! a guard check instead of a run, the old writes compose the guesses for
//! what follows, and the cells the previous run found volatile are holes
//! from round 1 on. The previous run's boundaries that still exist are
//! kept ([`Split::still_at`]), so its traces line up.
//!
//! The fixpoint (every guard holds on the composed entries, every hole
//! resolved) is the sequential run, so the output is identical. Each round
//! makes at least the first invalid region valid, so the number of rounds
//! is at most one plus the number of regions; a cap falls back to running
//! the rest sequentially.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::build::Build;
use crate::exec::Executor;
use crate::link::{Chunk, link};
use crate::machine::{Hole, Machine, Split, Step};
use crate::trace::{OnForce, Recording, Trace};

/// What a forced hole does, or no holes at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Holes {
    /// No holes: every read is a guard on the guessed value.
    Off,
    /// A forced hole suspends the region until its entry is exact.
    Suspend,
    /// A forced hole takes the guessed value under a guard.
    Speculate,
}

/// The switches of parallel rounds.
#[derive(Clone, Debug)]
pub struct Config {
    /// About how many regions to split the run into.
    pub parts: usize,
    pub holes: Holes,
    /// Rounds before the rest runs sequentially.
    pub max_rounds: usize,
    /// A cell written by at least this many regions becomes a hole.
    pub volatile_writers: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            parts: 8,
            holes: Holes::Speculate,
            max_rounds: 32,
            volatile_writers: 2,
        }
    }
}

/// What the rounds did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub regions: usize,
    /// Parallel phases.
    pub rounds: usize,
    /// Region runs over all rounds.
    pub runs: usize,
    /// Runs that suspended, and suspended regions resumed in a sweep.
    pub suspended: usize,
    pub resumed: usize,
    /// Cost units run in parallel phases and in sweeps (resumes, fallback).
    pub parallel_cost: u64,
    pub serial_cost: u64,
    /// The rest was run sequentially after the round cap.
    pub fallback: bool,
}

/// The result: the regions' traces with their hole values, the final
/// state, and statistics.
pub struct Outcome<M: Machine> {
    pub final_state: M,
    /// Region `i` enters at `bounds[i]`.
    pub bounds: Vec<M::Boundary>,
    pub traces: Vec<(Trace<M>, Vec<M::Value>)>,
    pub stats: Stats,
}

/// A previous run's regions, the guesses of a warm start (see the module
/// documentation): boundaries in program order, and a trace for each
/// region that ran from one boundary to the next.
#[derive(Clone)]
pub struct Warm<M: Machine> {
    bounds: Vec<M::Boundary>,
    traces: BTreeMap<M::Boundary, Trace<M>>,
}

impl<M: Machine> Warm<M> {
    /// The regions of a previous run of parallel rounds.
    #[must_use]
    pub fn from_outcome(o: &Outcome<M>) -> Self {
        let traces = o
            .traces
            .iter()
            .map(|(t, _)| (t.entry.clone(), t.clone()))
            .collect();
        Self {
            bounds: o.bounds.clone(),
            traces,
        }
    }

    /// The regions of a recorded build, merged into about `parts` regions
    /// of equal cost (the build's own regions are finer).
    ///
    /// # Panics
    ///
    /// If the build has no region.
    #[must_use]
    pub fn from_build(b: &Build<M>, parts: usize) -> Self {
        let all: Vec<&Trace<M>> = b.traces().collect();
        let total: u64 = all.iter().map(|t| t.cost).sum();
        let size = total.div_ceil(parts.max(1) as u64).max(1);
        let mut bounds = Vec::new();
        let mut traces = BTreeMap::new();
        let mut cur: Option<Trace<M>> = None;
        // The versions accumulating cells had at the current merged
        // region's entry, and have now (the last written, or the initial).
        let mut at_entry: BTreeMap<M::Cell, crate::hash::Version> = BTreeMap::new();
        let mut now: BTreeMap<M::Cell, crate::hash::Version> = BTreeMap::new();
        for t in all {
            cur =
                Some(match cur {
                    None => {
                        at_entry.clone_from(&now);
                        t.clone()
                    }
                    Some(c) => {
                        let entry =
                            |c: &M::Cell| {
                                Some(at_entry.get(c).copied().unwrap_or_else(|| {
                                    crate::hash::version_of(&b.initial().get(c))
                                }))
                            };
                        c.compose_with(t, &entry)
                            .expect("an entry version for every accumulating cell")
                    }
                });
            for (c, _, v) in &t.writes {
                if M::accumulates(c) {
                    now.insert(c.clone(), *v);
                }
            }
            if cur.as_ref().is_some_and(|c| c.cost >= size) {
                let c = cur.take().expect("just set");
                bounds.push(c.entry.clone());
                traces.insert(c.entry.clone(), c);
            }
        }
        if let Some(c) = cur {
            bounds.push(c.entry.clone());
            traces.insert(c.entry.clone(), c);
        }
        assert!(!bounds.is_empty(), "a build has at least one region");
        Self { bounds, traces }
    }

    /// How many regions.
    #[must_use]
    pub fn regions(&self) -> usize {
        self.bounds.len()
    }
}

impl<M: Machine> Outcome<M> {
    /// Link the output.
    pub fn output<E: Executor>(&self, exec: &E) -> Vec<u8> {
        let chunks: Vec<Chunk<'_, M>> = self
            .traces
            .iter()
            .map(|(t, env)| Chunk {
                effects: &t.effects,
                env,
                allocs: t.allocs,
            })
            .collect();
        link(&chunks, &self.final_state, exec)
    }
}

struct Susp<M: Machine> {
    m: M,
    rec: Recording<M>,
}

enum Slot<M: Machine> {
    Empty,
    Done(Trace<M>),
    Suspended(Box<Susp<M>>),
}

struct Job<M: Machine> {
    r: usize,
    m: M,
    stop: Option<M::Boundary>,
    holes: Vec<M::Cell>,
}

/// Run until `stop` (or the end). A finished run also returns the machine.
fn drive<M: Machine>(
    mut m: M,
    mut rec: Recording<M>,
    stop: Option<&M::Boundary>,
) -> (Slot<M>, Option<M>) {
    loop {
        match m.step(&mut rec) {
            Step::Continue => rec.cost += 1,
            Step::Candidate(_) => {
                rec.cost += 1;
                if stop.is_some_and(|s| *s == m.at()) {
                    m.prepare_cut(&mut rec);
                    let at = m.at();
                    return (Slot::Done(rec.finish(at)), Some(m));
                }
            }
            Step::Halt => {
                rec.cost += 1;
                m.prepare_cut(&mut rec);
                let at = m.at();
                return (Slot::Done(rec.finish(at)), Some(m));
            }
            Step::Suspended => return (Slot::Suspended(Box::new(Susp { m, rec })), None),
        }
    }
}

/// Hole values for `holes` in `e`, if every one is present.
fn env_in<'a, M: Machine + 'a>(
    holes: impl Iterator<Item = &'a M::Cell>,
    e: &M,
) -> Option<Vec<M::Value>> {
    holes.map(|c| e.get(c)).collect()
}

/// Run `initial` in parallel rounds.
///
/// # Panics
///
/// If `initial.split` returns no position.
pub fn run<M: Split, E: Executor>(initial: &M, cfg: &Config, exec: &E) -> Outcome<M> {
    run_warm(initial, cfg, exec, None)
}

/// Run `initial` in parallel rounds, seeded with a previous run's regions
/// when `warm` is given (see the module documentation). `None` is a cold
/// start, the same as [`run`].
///
/// # Panics
///
/// If `initial.split` returns no position.
#[allow(clippy::too_many_lines)] // sweep and round, kept whole
pub fn run_warm<M: Split, E: Executor>(
    initial: &M,
    cfg: &Config,
    exec: &E,
    warm: Option<&Warm<M>>,
) -> Outcome<M> {
    let bounds = match warm {
        Some(w) => core::iter::once(initial.at())
            .chain(
                w.bounds
                    .iter()
                    .skip(1)
                    .filter(|b| **b != initial.at() && initial.still_at(b))
                    .cloned(),
            )
            .collect(),
        None => initial.split(cfg.parts.max(1)),
    };
    assert!(!bounds.is_empty(), "split gives at least the start");
    let n = bounds.len();
    let stop_of = |r: usize| bounds.get(r + 1).cloned();
    let on_force = if cfg.holes == Holes::Speculate {
        OnForce::Speculate
    } else {
        OnForce::Suspend
    };
    // A warm start seeds each region with the previous run's trace from
    // the same entry to the same exit; the first sweep validates them.
    let mut slots: Vec<Slot<M>> = (0..n)
        .map(|r| {
            warm.and_then(|w| w.traces.get(&bounds[r]))
                .filter(|t| stop_of(r).is_none_or(|s| s == t.exit))
                .map_or(Slot::Empty, |t| Slot::Done(t.clone()))
        })
        .collect();
    let mut envs: Vec<Vec<M::Value>> = (0..n).map(|_| Vec::new()).collect();
    let mut stats = Stats {
        regions: n,
        ..Stats::default()
    };
    // Regions before `exact_upto` are final; `exact_state` is the exact
    // state at their end.
    let mut exact_upto = 0;
    let mut exact_state = initial.clone();
    exact_state.seek(&bounds[0]);
    let mut fallback: Vec<Trace<M>> = Vec::new();
    let final_state = loop {
        let volatile = if cfg.holes == Holes::Off {
            Vec::new()
        } else {
            let mut count: BTreeMap<&M::Cell, usize> = BTreeMap::new();
            for s in &slots[exact_upto..] {
                match s {
                    Slot::Done(t) => t
                        .writes
                        .iter()
                        .for_each(|(c, _, _)| *count.entry(c).or_default() += 1),
                    Slot::Suspended(s) => s
                        .rec
                        .writes()
                        .for_each(|(c, _)| *count.entry(c).or_default() += 1),
                    Slot::Empty => {}
                }
            }
            count
                .into_iter()
                .filter(|(_, k)| *k >= cfg.volatile_writers)
                .map(|(c, _)| c.clone())
                .collect()
        };
        let mut e = exact_state.clone();
        let mut exact = true;
        let mut jobs = Vec::new();
        let mut done = false;
        let from = exact_upto;
        for r in from..n {
            let stop = stop_of(r);
            if exact && matches!(slots[r], Slot::Suspended(_)) {
                let Slot::Suspended(mut s) = core::mem::replace(&mut slots[r], Slot::Empty) else {
                    unreachable!()
                };
                // The entry is exact: resolve the holes and go on.
                if s.rec.holds(&e) && s.rec.resolve(&e, &mut s.m) {
                    let before = s.rec.cost;
                    let (slot, _) = drive(s.m, s.rec, stop.as_ref());
                    if let Slot::Done(t) = &slot {
                        stats.serial_cost += t.cost - before;
                    }
                    stats.resumed += 1;
                    if let Slot::Done(_) = slot {
                        slots[r] = slot;
                    }
                }
            }
            let valid = match &slots[r] {
                Slot::Done(t) if t.entry == e.at() && t.holds(&e) => env_in(t.holes.iter(), &e),
                _ => None,
            };
            if let Some(env) = valid {
                let Slot::Done(t) = &slots[r] else {
                    unreachable!()
                };
                let early = stop.as_ref().is_some_and(|s| *s != t.exit);
                if early && exact {
                    // The region ended somewhere else (it halted early):
                    // leave the rest to the sequential fallback.
                    exact_upto = r;
                    exact_state = e.clone();
                    break;
                }
                t.apply(&mut e, &env);
                envs[r] = env;
                if early {
                    e.seek(stop.as_ref().expect("checked"));
                }
                if exact && r + 1 == n {
                    done = true;
                }
                continue;
            }
            let first = exact;
            if exact {
                exact = false;
                exact_upto = r;
                exact_state = e.clone();
            }
            let mut m = e.clone();
            m.seek(&bounds[r]);
            let holes = if first {
                Vec::new()
            } else {
                volatile
                    .iter()
                    .filter(|c| e.get(c).is_some())
                    .cloned()
                    .collect()
            };
            jobs.push(Job {
                r,
                m,
                stop: stop.clone(),
                holes,
            });
            // The old writes still compose the guess for what follows.
            match &slots[r] {
                Slot::Done(t) => {
                    if let Some(env) = env_in(t.holes.iter(), &e) {
                        t.apply(&mut e, &env);
                    }
                }
                Slot::Suspended(s) => {
                    if let Some(env) = env_in(s.rec.hole_cells(), &e) {
                        let look = |h: Hole| env.get(h.0 as usize).cloned();
                        for (c, v) in s.rec.writes() {
                            e.set(c, Some(M::subst_value(v, &look)));
                        }
                    }
                }
                Slot::Empty => {}
            }
            if let Some(s) = &stop {
                e.seek(s);
            }
        }
        if done {
            exact_upto = n;
            break e;
        }
        if jobs.is_empty() || stats.rounds >= cfg.max_rounds {
            // The round cap, or a region that ended early: run the rest
            // sequentially from the exact state.
            // Region by region, so that a warm start from this run finds
            // a trace at every boundary.
            stats.fallback = true;
            let mut m = exact_state.clone();
            if exact_upto < n {
                m.seek(&bounds[exact_upto]);
            }
            let mut r = exact_upto;
            loop {
                let stop = stop_of(r);
                let rec = Recording::new(m.at(), OnForce::Suspend);
                let (Slot::Done(t), Some(next)) = drive(m, rec, stop.as_ref()) else {
                    unreachable!("no holes, nothing to suspend on")
                };
                stats.serial_cost += t.cost;
                let ended = stop.is_none_or(|s| s != t.exit);
                fallback.push(t);
                m = next;
                r += 1;
                if ended || r >= n {
                    break;
                }
            }
            break m;
        }
        stats.rounds += 1;
        let ran = exec.map(jobs, |job| {
            let mut m = job.m;
            let mut rec = Recording::new(m.at(), on_force);
            for c in &job.holes {
                if let Some(g) = m.get(c) {
                    rec.plant(&mut m, c, g);
                }
            }
            (job.r, drive(m, rec, job.stop.as_ref()).0)
        });
        for (r, slot) in ran {
            stats.runs += 1;
            match &slot {
                Slot::Done(t) => stats.parallel_cost += t.cost,
                Slot::Suspended(s) => {
                    stats.suspended += 1;
                    stats.parallel_cost += s.rec.cost;
                }
                Slot::Empty => {}
            }
            slots[r] = slot;
        }
    };
    let mut traces: Vec<(Trace<M>, Vec<M::Value>)> = slots
        .into_iter()
        .zip(envs)
        .take(exact_upto)
        .map(|(s, env)| match s {
            Slot::Done(t) => (t, env),
            _ => unreachable!("regions before the exact point are done"),
        })
        .collect();
    traces.extend(fallback.into_iter().map(|t| (t, Vec::new())));
    Outcome {
        final_state,
        bounds,
        traces,
        stats,
    }
}
