//! Evaluation: dirty nodes run lowest position first against the current
//! world; a run either commits (a new world) or parks on what it waits for.
//! Quiet, the loop-back rounds and the final phase follow.

use std::collections::BTreeSet;

use super::build::Builder;
use super::ctx::{Spec, Wait};
use super::engine::Engine;
use super::ptree::NoMeasure;
use super::world::{Id, Pos, Read, Res, St, World};
use crate::api::{CapPolicy, EditSource, LoopStart, EvalError, Executor, NoEdits, NodeId, Report, Scheduler, Sequential};
use crate::values::{Field, FxHashMap, FxHashSet, ValueId};
use crate::{Ctx, Greedy};

/// What a run gave.
enum Outcome {
    Done(Run),
    Stopped(Run, Wait),
    Failed(EvalError),
}

/// A run's reads and writes, owned.
struct Run {
    res: Res,
    kids: Vec<Spec>,
    conts: Vec<Spec>,
    kept: Option<Box<dyn std::any::Any + Send>>,
    apps: Vec<(Id, crate::api::OpId, std::sync::Arc<[ValueId]>)>,
    env: Vec<(Field, ValueId)>,
    pset_wait: bool,
    predicted: u32,
    /// It read a prediction or a definer in work.
    guessed: bool,
    fuel_out: bool,
    range_wait: Option<(Field, Id, Id)>,
    narrowed: Option<super::world::May>,
}

/// The evaluation's scheduling state.
#[derive(Default)]
struct Sched {
    queue: BTreeSet<(u32, Pos, Id)>,
    queued: FxHashSet<Id>,
    /// Who waits on what: subject -> waiting nodes.
    waiters: FxHashMap<Id, Vec<Id>>,
    /// Each parked node's wait.
    parked: FxHashMap<Id, Wait>,
    /// Nodes waiting on an environment field.
    env: FxHashMap<Id, Field>,
    runs: FxHashMap<Id, u32>,
    /// Why each node's last runs were needed (oldest first, a few).
    why: FxHashMap<Id, std::collections::VecDeque<crate::api::Cause>>,
    /// Each node's run time (all its attempts).
    time: FxHashMap<Id, std::time::Duration>,
    /// Nodes whose speculative run ran out of fuel: they run in full.
    no_fuel: FxHashSet<Id>,
    /// How often each node's result was refuted.
    refutes: FxHashMap<Id, u32>,
    /// Nodes whose committed result rests on a guess.
    guessed: FxHashSet<Id>,
    /// Nodes whose run stopped on an accumulator range, with the range.
    range_wait: FxHashMap<Id, (Field, Id, Id)>,
}

impl Sched {
    /// A schedule for another world: the run counts and causes go on.
    fn fresh(&mut self) -> Sched {
        Sched {
            runs: std::mem::take(&mut self.runs),
            why: std::mem::take(&mut self.why),
            time: std::mem::take(&mut self.time),
            refutes: std::mem::take(&mut self.refutes),
            guessed: std::mem::take(&mut self.guessed),
            ..Sched::default()
        }
    }
}

/// A rerun reason as a cause of a run.
fn cause_of(r: crate::api::Rerun) -> crate::api::Cause {
    use crate::api::{Cause, Rerun};
    match r {
        Rerun::Loop { field } => Cause::Loop { field },
        Rerun::Changed { field, .. } => Cause::Changed { field, site: None },
        Rerun::Rebind { field } => Cause::Rebind { field, from: None, to: None },
        Rerun::Environment { field } => Cause::Environment { field },
        Rerun::Output { of } => Cause::Output { of },
        _ => Cause::Structure,
    }
}

/// Heights of the runs of one evaluation: each run's height and the link
/// to the run it stood on.
#[derive(Default)]
struct Heights {
    on: bool,
    h: FxHashMap<Id, (u32, Option<Id>, crate::api::Cause)>,
    /// What each parked node last waited on.
    woke: FxHashMap<Id, (Id, Option<Field>)>,
}

impl Engine {
    /// Evaluates on the calling thread, in steps.
    ///
    /// # Errors
    ///
    /// As [`Engine::evaluate`].
    pub fn evaluate_sequential(&mut self) -> Result<Report, EvalError> {
        self.evaluate(&mut Greedy::default(), &mut Sequential)
    }

    /// Evaluates to the fixed point.
    ///
    /// # Errors
    ///
    /// A cycle, a conflicting publication, a runaway node, a panic, a limit.
    pub fn evaluate(&mut self, sched: &mut dyn Scheduler, exec: &mut dyn Executor) -> Result<Report, EvalError> {
        self.evaluate_absorbing(sched, exec, &mut NoEdits)
    }

    /// Evaluates, taking in the edits `src` supplies.
    ///
    /// # Errors
    ///
    /// As [`Engine::evaluate`].
    pub fn evaluate_absorbing(
        &mut self,
        sched: &mut dyn Scheduler,
        exec: &mut dyn Executor,
        src: &mut dyn EditSource,
    ) -> Result<Report, EvalError> {
        let r = self.evaluate_inner(sched, exec, src);
        self.core.prune_texts();
        if r.is_err() {
            // The round worlds are gone: the next evaluation starts the
            // rounds again from the current world.
            self.rounds.clear();
            self.round_dirty.clear();
            self.last_is_round = false;
        }
        r
    }

    fn evaluate_inner(
        &mut self,
        sched: &mut dyn Scheduler,
        exec: &mut dyn Executor,
        src: &mut dyn EditSource,
    ) -> Result<Report, EvalError> {
        let t0 = std::time::Instant::now();
        let mut rep = Report::default();
        if let Some(v) = src.absorb(self) {
            self.version = v;
            rep.edits_applied.push((v, 0));
        }
        if let Some(e) = self.hook_error() {
            return Err(e);
        }
        // The provider is asked again about fields it did not know.
        self.core.env.write().unwrap_or_else(std::sync::PoisonError::into_inner).unknown.clear();
        self.core.env_provided.store(0, std::sync::atomic::Ordering::Relaxed);
        let mut s = Sched::default();
        // Per-round worlds: each round starts from its own old world (the
        // edits already applied to it), the final phase from the old final
        // world (the current one, unless it is the last round's).
        let per_round = self.loop_start != LoopStart::Previous && !self.core.loops.is_empty();
        self.ev_olds = std::mem::take(&mut self.rounds)
            .into_iter()
            .zip(std::mem::take(&mut self.round_dirty))
            .collect();
        self.ev_done.clear();
        self.ev_fin = None;
        let pending = std::mem::take(&mut self.pending);
        // From the old rounds: their count, and whether some round's
        // loop-back values moved against its old world's.
        let had_old = per_round && (self.last_is_round || !self.ev_olds.is_empty());
        let old_rounds = self.ev_olds.len() as u64 + u64::from(self.last_is_round);
        let mut moved_vs_old = false;
        if had_old {
            let cur = std::mem::take(&mut self.w);
            if self.last_is_round {
                self.ev_olds.push_back((cur, pending));
            } else {
                self.ev_fin = Some((cur, pending));
            }
        } else {
            self.ev_olds.clear();
            if per_round {
                // The rounds start from a final world: its final nodes are
                // held again.
                let w = std::mem::take(&mut self.w);
                let mut b = Builder::new(&self.core, w);
                b.hold_finals();
                let m = b.finish();
                self.w = m.w;
                for x in m.dirty {
                    self.enqueue(&mut s, x);
                }
            }
            self.start_world(&mut s, &pending);
        }
        let mut round_runs = 0u64;
        let mut rounds: u64;
        let mut ran: FxHashSet<Id> = FxHashSet::default();
        let mut cut: FxHashSet<Id> = FxHashSet::default();
        let deadline = self.limits.time.map(|d| t0 + d);
        let mut hs = Heights {
            on: self.heights,
            ..Heights::default()
        };
        let mut from_old = had_old;
        // A pass runs the rounds from the first; an edit absorbed in the
        // middle of per-round rounds starts another.
        'pass: loop {
            self.ev_prev = None;
            if per_round {
                if let Some((w, d)) = self.ev_olds.pop_front() {
                    self.w = w;
                    s = s.fresh();
                    self.start_world(&mut s, &d);
                }
                let init: Vec<(Field, ValueId)> = self.core.loops.iter().map(|(&f, &v)| (f, v)).collect();
                moved_vs_old |= self.set_loops(&mut s, &init) && had_old;
            }
            rounds = 1;
            loop {
                let restart = self.drain(&mut s, &mut rep, &mut ran, &mut cut, deadline, &mut hs, src, per_round, rounds, sched, exec)?;
                if restart {
                    // Every world again, in order, as the bases of a new pass.
                    let cur = std::mem::take(&mut self.w);
                    let mut all: std::collections::VecDeque<(World, Vec<Id>)> = std::mem::take(&mut self.ev_done).into();
                    all.push_back((cur, Vec::new()));
                    all.extend(std::mem::take(&mut self.ev_olds));
                    self.ev_olds = all;
                    from_old = true;
                    rep.restarts += 1;
                    continue 'pass;
                }
                // Quiet: edits first.
                let before = self.edits;
                if let Some(v) = super::core::prof::time(8, || src.absorb(self)) {
                    self.version = v;
                    rep.edits_applied.push((v, rounds));
                }
                if self.edits != before {
                    if let Some(e) = self.hook_error() {
                        return Err(e);
                    }
                    if per_round {
                        let cur = std::mem::take(&mut self.w);
                        let mut all: std::collections::VecDeque<(World, Vec<Id>)> = std::mem::take(&mut self.ev_done).into();
                        all.push_back((cur, Vec::new()));
                        all.extend(std::mem::take(&mut self.ev_olds));
                        self.ev_olds = all;
                        from_old = true;
                        rep.restarts += 1;
                        continue 'pass;
                    }
                    for x in std::mem::take(&mut self.pending) {
                        self.enqueue(&mut s, x);
                    }
                    continue;
                }
                // Quiet with nodes stuck: the readers that guessed past what
                // they will make run again and wait.
                if super::core::prof::time(7, || self.unguess(&mut s)) {
                    continue;
                }
                // Quiet: a cycle through outputs is an error.
                if let Some(c) = self.cycle(&s) {
                    return Err(EvalError::Cycle(c.into_iter().map(NodeId).collect()));
                }
                // Loop-backs runs declared: taken in now, their readers run.
                let later = std::mem::take(&mut *super::core::lock(&self.core.loops_later));
                if !later.is_empty() {
                    for (f, init) in later {
                        self.declare_loopback(f, init);
                    }
                    for id in std::mem::take(&mut self.pending) {
                        self.enqueue(&mut s, id);
                    }
                    continue;
                }
                // The next round's loop-back values.
                let next = self.publications()?;
                let moved: Vec<Field> = next.iter().filter(|(f, x)| self.w.loops.get(f) != Some(x)).map(|&(f, _)| f).collect();
                if !moved.is_empty() && !self.loop_readers(&moved).is_empty() {
                    if rounds >= self.max_rounds {
                        match self.cap_policy {
                            CapPolicy::Error => return Err(EvalError::NonConvergence { rounds }),
                            CapPolicy::Finalize => rep.capped = Some(moved),
                        }
                    } else {
                        self.round_done(&mut rep, &mut round_runs, from_old);
                        rounds += 1;
                        if per_round {
                            if let Some((w, d)) = self.ev_olds.pop_front() {
                                let done = std::mem::replace(&mut self.w, w);
                                self.ev_prev = Some(done.clone());
                                self.ev_done.push((done, Vec::new()));
                                s = s.fresh();
                                self.start_world(&mut s, &d);
                            } else {
                                self.ev_done.push((self.w.clone(), Vec::new()));
                                from_old = false;
                                moved_vs_old |= had_old;
                            }
                        }
                        moved_vs_old |= self.set_loops(&mut s, &next) && from_old;
                        continue;
                    }
                } else {
                    // Moved with no reader: the values the final phase sees.
                    self.set_loops(&mut s, &next);
                }
                // The final phase: held nodes run once, in the final world.
                let held = self.w.nodes.iter().any(|(_, n)| n.st == St::Held);
                self.round_done(&mut rep, &mut round_runs, from_old);
                if !held {
                    self.last_is_round = true;
                    break;
                }
                self.last_is_round = false;
                let loops: Vec<(Field, ValueId)> = self.w.loops.iter().map(|(&f, &v)| (f, v)).collect();
                let round_w = self.w.clone();
                if per_round {
                    if let Some((w, d)) = self.ev_fin.take() {
                        let done = std::mem::replace(&mut self.w, w);
                        self.ev_prev = Some(done.clone());
                        self.ev_done.push((done, Vec::new()));
                        s = s.fresh();
                        self.start_world(&mut s, &d);
                        self.set_loops(&mut s, &loops);
                    } else {
                        self.ev_done.push((self.w.clone(), Vec::new()));
                    }
                }
                // Held nodes run; final nodes the final phase itself places
                // (held when made) run in turn, until none is left.
                rep.final_nodes_run += 1;
                loop {
                    let held: Vec<Id> = self.w.nodes.iter().filter(|(_, n)| n.st == St::Held).map(|(&id, _)| id).collect();
                    if held.is_empty() {
                        break;
                    }
                    let w = std::mem::take(&mut self.w);
                    let mut b = Builder::new(&self.core, w);
                    for &id in &held {
                        if let Some(mut n) = b.w.node(id).cloned() {
                            n.st = St::Dirty;
                            b.w.put(id, n);
                        }
                    }
                    self.w = b.finish().w;
                    for id in held {
                        self.enqueue(&mut s, id);
                    }
                    let restart = self.drain(&mut s, &mut rep, &mut ran, &mut cut, deadline, &mut hs, src, per_round, rounds, sched, exec)?;
                    if restart {
                        let cur = std::mem::take(&mut self.w);
                        let mut all: std::collections::VecDeque<(World, Vec<Id>)> = std::mem::take(&mut self.ev_done).into();
                        self.ev_fin = Some((cur, Vec::new()));
                        all.extend(std::mem::take(&mut self.ev_olds));
                        self.ev_olds = all;
                        from_old = true;
                        rep.restarts += 1;
                        continue 'pass;
                    }
                }
                // The final phase moved a loop-back: the final world publishes
                // another value than the last round's world did.
                let last = self.ev_done.last().map_or_else(|| round_w.clone(), |d| d.0.clone());
                let before = publications_of(&self.core, &last)?;
                let after = publications_of(&self.core, &self.w)?;
                if let Some(&(f, _)) = after.iter().find(|p| !before.contains(p)) {
                    let mut by: Vec<(u64, Id)> = self
                        .w
                        .nodes
                        .iter()
                        .filter(|(id, n)| {
                            let now = n.res.as_ref().filter(|_| n.st == St::Done).and_then(|r| r.pubs.iter().find(|p| p.0 == f).map(|p| p.1));
                            let was = last
                                .node(**id)
                                .filter(|o| o.st == St::Done)
                                .and_then(|o| o.res.as_ref())
                                .and_then(|r| r.pubs.iter().find(|p| p.0 == f).map(|p| p.1));
                            now.is_some() && now != was
                        })
                        .map(|(&id, n)| (n.pos, id))
                        .collect();
                    by.sort_unstable();
                    return Err(EvalError::FinalMovedLoop {
                        node: NodeId(by.first().map_or(0, |b| b.1)),
                        field: f,
                    });
                }
                break;
            }
            break;
        }
        if per_round {
            let done = std::mem::take(&mut self.ev_done);
            self.round_dirty = vec![Vec::new(); done.len()];
            self.rounds = done.into_iter().map(|d| d.0).collect();
        }
        self.ev_olds.clear();
        self.ev_fin = None;
        self.ev_prev = None;
        // Marked nodes settle at the end (loop-backs) or as passed.
        self.report_settled(src, &mut rep, true);
        let blocked = self.env_blocked(&s);
        // Waits on nodes that stay: nothing will wake them.
        let mut stuck: Vec<crate::api::Waiter> = s
            .parked
            .iter()
            .filter(|(id, _)| {
                self.w
                    .node(**id)
                    .is_some_and(|n| super::world::in_work(n.st) && self.w.units.get(&n.unit).is_none_or(|un| un.app == super::world::NONE))
                    && !blocked.contains(*id)
            })
            .filter_map(|(&id, w)| {
                let on = match w {
                    Wait::Node(x, f) => crate::api::WaitOn::Node(NodeId(*x), *f),
                    Wait::Child(k) => {
                        let c = self.w.node(id).and_then(|n| n.res.as_ref()).and_then(|r| r.kids.get(*k).copied())?;
                        crate::api::WaitOn::Node(NodeId(c), None)
                    }
                    Wait::Env(_) => return None,
                };
                Some(crate::api::Waiter { node: NodeId(id), on })
            })
            .collect();
        if !stuck.is_empty() {
            stuck.sort_by_key(|w| w.node);
            if super::core::debug() {
                for w in &stuck {
                    if let crate::api::WaitOn::Node(x, f) = w.on {
                        let xn = self.w.node(x.0);
                        eprintln!(
                            "stuck {:?} on {:?} {:?}: subject st {:?} parked {:?} text {:?}",
                            w.node,
                            x,
                            f,
                            xn.map(|n| n.st),
                            s.parked.get(&x.0),
                            xn.map(|n| self.core.values.render(n.text.args[0]))
                        );
                    }
                }
            }
            rep.stuck_subjects = self.stuck_subjects(&s, &stuck);
            return Err(EvalError::NoProgress {
                rounds: self.limits.idle_rounds.saturating_add(1),
                waiters: stuck,
                report: Box::new(rep),
            });
        }
        // Environment waits that stay.
        let mut env: Vec<(NodeId, Field)> = s.env.iter().map(|(&n, &f)| (NodeId(n), f)).collect();
        env.sort_by_key(|e| e.0);
        rep.env_stalled = s
            .parked
            .keys()
            .filter(|&&id| self.w.node(id).is_some_and(|n| super::world::in_work(n.st)) && blocked.contains(&id))
            .count() as u64;
        rep.env_waits = env;
        rep.rounds = rounds;
        rep.nodes_run = ran.len() as u64;
        rep.cutoffs = cut.len() as u64;
        rep.created = std::mem::take(&mut self.created);
        rep.freed = std::mem::take(&mut self.freed);
        rep.rebinds = std::mem::take(&mut self.rebinds);
        rep.edits = self.edits;
        rep.version = self.version;
        if let Some(t) = rep.round_times.last_mut() {
            t.wall_ns = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
        }
        rep.quick_held = had_old && !moved_vs_old && rounds == old_rounds;
        // (Every per-round evaluation after the first runs the rounds again
        // from the initial values.)
        rep.restarts += u64::from(had_old);
        rep.env_provided = self.core.env_provided.load(std::sync::atomic::Ordering::Relaxed);
        let mut times: Vec<(NodeId, std::time::Duration)> = std::mem::take(&mut s.time).into_iter().map(|(id, t)| (NodeId(id), t)).collect();
        times.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        rep.run_times = times;
        rep.refuted_nodes = s.refutes.len() as u64;
        rep.nodes_conservative = rep.refuted_nodes;
        rep.refutations_max = s.refutes.values().copied().max().unwrap_or(0);
        rep.refuted_by_field.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.0.raw().cmp(&b.0.0.raw())));
        self.chain = if hs.on { chain_of(&hs) } else { Vec::new() };
        rep.critical_path = self.chain.first().map_or(0, |c| c.1);
        rep.realised_critical_path = self.chain.first().map_or(0, |c| c.1 + 1);
        let mut causes: Vec<(NodeId, crate::api::Rerun)> = std::mem::take(&mut *super::core::lock(&self.core.causes))
            .into_iter()
            .map(|(id, c)| (NodeId(id), c))
            .collect();
        causes.sort_by_key(|c| c.0);
        rep.rerun_causes = causes;
        super::core::prof::dump();
        Ok(rep)
    }

    fn enqueue(&self, s: &mut Sched, id: Id) {
        let Some(n) = self.w.node(id) else { return };
        if n.st != St::Dirty || s.queued.contains(&id) {
            return;
        }
        s.queued.insert(id);
        s.queue.insert((n.unit, n.pos, id));
    }

    /// Runs dirty nodes until none is ready.
    #[allow(clippy::too_many_arguments)]
    fn drain(
        &mut self,
        s: &mut Sched,
        rep: &mut Report,
        ran: &mut FxHashSet<Id>,
        cut: &mut FxHashSet<Id>,
        deadline: Option<std::time::Instant>,
        hs: &mut Heights,
        src: &mut dyn EditSource,
        per_round: bool,
        rounds: u64,
        sched: &mut dyn Scheduler,
        exec: &mut dyn Executor,
    ) -> Result<bool, EvalError> {
        // (The flow is opt-in: merging in the workers under one lock cost
        // more than it saved, 2026-10-20: para4000 j2-j48 ~700 ms against
        // steps ~350-430 ms.)
        if exec.parallelism() > 1 && std::env::var_os("CLEAN_FLOW").is_some() {
            return self.drain_flow(s, rep, ran, cut, deadline, hs, src, per_round, rounds, sched, exec);
        }
        if exec.parallelism() > 1 || exec.steps() {
            return self.drain_steps(s, rep, ran, cut, deadline, hs, src, per_round, rounds, sched, exec);
        }
        let mut idle: u32 = 0;
        while let Some(k) = s.queue.first().copied() {
            rep.max_ready = rep.max_ready.max(s.queue.len());
            self.check_deadline(s, rep, deadline)?;
            s.queue.pop_first();
            let id = k.2;
            let Some(resume) = super::core::prof::time(6, || self.prepare(s, rep, k)) else { continue };
            if resume.is_none()
                && let Some(run) = self.reuse(id)
            {
                self.unrun(rep, id);
                self.apply(s, rep, ran, cut, hs, id, Outcome::Done(run), true)?;
                continue;
            }
            // (Through the executor: the client's worker runs it.)
            let slot: std::sync::Mutex<Option<(Outcome, std::time::Duration)>> = std::sync::Mutex::new(None);
            let resume = std::sync::Mutex::new(resume);
            {
                let (core, w) = (&self.core, &self.w);
                let run = |_: usize| {
                    #[cfg(not(target_arch = "wasm32"))]
                    let t = std::time::Instant::now();
                    let o = super::core::prof::time(0, || run_job(core, w, id, super::core::lock(&resume).take(), false, false, None));
                    #[cfg(not(target_arch = "wasm32"))]
                    let d = t.elapsed();
                    #[cfg(target_arch = "wasm32")]
                    let d = std::time::Duration::ZERO;
                    *super::core::lock(&slot) = Some((o, d));
                    crate::api::RunResult(())
                };
                let mut out = Vec::with_capacity(1);
                exec.execute(1, &run, &mut out);
            }
            let Some((outcome, d)) = slot.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner) else {
                // No result: it runs again; too many in a row is no progress.
                if let Some(r) = resume.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner) {
                    self.kept.insert(id, r);
                }
                self.enqueue(s, id);
                idle += 1;
                rep.steps += 1;
                if idle > self.limits.idle_rounds {
                    let waiters = s
                        .queue
                        .iter()
                        .map(|k| crate::api::Waiter {
                            node: NodeId(k.2),
                            on: crate::api::WaitOn::Queued,
                        })
                        .collect();
                    return Err(EvalError::NoProgress {
                        rounds: idle,
                        waiters,
                        report: Box::new(rep.clone()),
                    });
                }
                continue;
            };
            idle = 0;
            *s.time.entry(id).or_default() += d;
            let applied = matches!(outcome, Outcome::Done(_));
            super::core::prof::time(1, || self.apply(s, rep, ran, cut, hs, id, outcome, true))?;
            rep.steps += 1;
            if applied {
                width(rep, 1);
                if self.step_absorb(s, rep, src, per_round, rounds)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Steps: each step's batch, chosen by the scheduler from the nodes to
    /// run in text order, runs through the executor against one snapshot;
    /// the results merge in text order, a run committing only when every
    /// read it made still holds (else it runs again).
    #[allow(clippy::too_many_arguments)]
    fn drain_steps(
        &mut self,
        s: &mut Sched,
        rep: &mut Report,
        ran: &mut FxHashSet<Id>,
        cut: &mut FxHashSet<Id>,
        deadline: Option<std::time::Instant>,
        hs: &mut Heights,
        src: &mut dyn EditSource,
        per_round: bool,
        rounds: u64,
        sched: &mut dyn Scheduler,
        exec: &mut dyn Executor,
    ) -> Result<bool, EvalError> {
        let window = (exec.parallelism().max(1) * 8).max(64);
        let mut idle: u32 = 0;
        let predict = sched.speculation_budget() > 0;
        while !s.queue.is_empty() {
            rep.max_ready = rep.max_ready.max(s.queue.len());
            self.check_deadline(s, rep, deadline)?;
            let Some((mut chosen, mut resumes, fueled)) = self.select_batch(s, rep, ran, cut, hs, window, predict, sched, &FxHashSet::default())? else { continue };
            // Run against the snapshot; the results merge in text order
            // after the batch (merging in the workers as results came only
            // added contention: 2026-10-20, para4000 j8 341 -> 430 ms).
            chosen.sort_by_key(|&id| self.w.node(id).map(|n| (n.unit, n.pos)));
            let snap = self.w.clone();
            let jobs: Vec<(Id, std::sync::Mutex<Option<(Box<dyn std::any::Any + Send>, Vec<Read>)>>)> = chosen
                .iter()
                .map(|&id| (id, std::sync::Mutex::new(resumes.remove(&id).flatten())))
                .collect();
            let fuel = sched.fuel();
            // A node refuted this evaluation runs on committed values only:
            // no predictions, and a definer in work is waited for.
            let careful: FxHashSet<Id> = chosen.iter().copied().filter(|x| s.refutes.contains_key(x)).collect();
            let slots: Vec<std::sync::Mutex<Option<(Outcome, std::time::Duration)>>> = (0..jobs.len()).map(|_| std::sync::Mutex::new(None)).collect();
            let core = std::sync::Arc::clone(&self.core);
            {
                let run = |i: usize| {
                    let (id, resume) = &jobs[i];
                    let resume = super::core::lock(resume).take();
                    #[cfg(not(target_arch = "wasm32"))]
                    let t = std::time::Instant::now();
                    let o = super::core::prof::time(0, || run_job(&core, &snap, *id, resume, predict && !careful.contains(id), careful.contains(id), fueled.contains(id).then_some(fuel)));
                    #[cfg(not(target_arch = "wasm32"))]
                    let d = t.elapsed();
                    #[cfg(target_arch = "wasm32")]
                    let d = std::time::Duration::ZERO;
                    *super::core::lock(&slots[i]) = Some((o, d));
                    crate::api::RunResult(())
                };
                let mut out = Vec::with_capacity(jobs.len());
                exec.execute(jobs.len(), &run, &mut out);
            }
            // The field writes of the commits that change no structure are
            // recorded and made together, in parallel ([`super::batch`]).
            self.defer = Some(super::batch::Pending::default());
            let mut m = Merge {
                eng: &mut *self,
                s: &mut *s,
                rep: &mut *rep,
                ran: &mut *ran,
                cut: &mut *cut,
                hs: &mut *hs,
                next: 0,
                applied: 0,
                parked: 0,
                err: None,
            };
            m.drain(&snap, &jobs, &slots, exec);
            m.flush(exec);
            m.eng.defer = None;
            let (applied, parked, err) = (m.applied, m.parked, m.err.take());
            drop(m);
            if let Some(e) = err {
                return Err(e);
            }
            rep.steps += 1;
            width(rep, applied);
            // Steps that apply and park nothing, too many in a row: no
            // progress.
            idle = if applied == 0 && parked == 0 && !s.queue.is_empty() { idle + 1 } else { 0 };
            if idle > self.limits.idle_rounds {
                let waiters = s
                    .queue
                    .iter()
                    .map(|k| crate::api::Waiter {
                        node: NodeId(k.2),
                        on: crate::api::WaitOn::Queued,
                    })
                    .collect();
                return Err(EvalError::NoProgress {
                    rounds: idle,
                    waiters,
                    report: Box::new(rep.clone()),
                });
            }
            if self.step_absorb(s, rep, src, per_round, rounds)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Runs dirty nodes on the executor's workers without steps: a worker
    /// that finishes merges what finished (validated against the world as
    /// it is), takes the next ready node and runs it on the world then; edits
    /// are taken in between epochs of a few hundred merges.
    #[allow(clippy::too_many_arguments)]
    fn drain_flow(
        &mut self,
        s: &mut Sched,
        rep: &mut Report,
        ran: &mut FxHashSet<Id>,
        cut: &mut FxHashSet<Id>,
        deadline: Option<std::time::Instant>,
        hs: &mut Heights,
        src: &mut dyn EditSource,
        per_round: bool,
        rounds: u64,
        sched: &mut dyn Scheduler,
        exec: &mut dyn Executor,
    ) -> Result<bool, EvalError> {
        let par = exec.parallelism().max(1);
        let window = (par * 8).max(64);
        let predict = sched.speculation_budget() > 0;
        let fuel = sched.fuel();
        let mut idle: u32 = 0;
        while !s.queue.is_empty() {
            rep.max_ready = rep.max_ready.max(s.queue.len());
            self.check_deadline(s, rep, deadline)?;
            let flow = std::sync::Mutex::new(Flow {
                m: Merge {
                    eng: &mut *self,
                    s: &mut *s,
                    rep: &mut *rep,
                    ran: &mut *ran,
                    cut: &mut *cut,
                    hs: &mut *hs,
                    next: 0,
                    applied: 0,
                    parked: 0,
                    err: None,
                },
                sched: crate::sync::SendMut::new(&mut *sched),
                buf: std::collections::VecDeque::new(),
                fueled: FxHashSet::default(),
                inflight: FxHashSet::default(),
                done: Vec::new(),
                merged: 0,
                stop: false,
            });
            let cv = std::sync::Condvar::new();
            let budget = 256usize;
            let run = |_: usize| {
                let mut g = super::core::lock(&flow);
                loop {
                    // Merge what finished, in text order: a result waits
                    // for the runs before it (as a step's merge would).
                    let key = |w: &World, x: Id| w.node(x).map_or((u32::MAX, Pos::MAX), |n| (n.unit, n.pos));
                    let mut done = std::mem::take(&mut g.done);
                    let mut now: Vec<(Id, World, Outcome, std::time::Duration)> = Vec::new();
                    {
                        let f = &mut *g;
                        let w = &f.m.eng.w;
                        let first_running = f.inflight.iter().map(|&x| key(w, x)).min();
                        done.sort_by_key(|r| std::cmp::Reverse(key(w, r.0)));
                        while done.last().is_some_and(|r| first_running.is_none_or(|fr| key(w, r.0) < fr)) {
                            now.push(done.pop().expect("checked"));
                        }
                        f.done = done;
                    }
                    for (id, snap, o, d) in now {
                        if g.m.err.is_some() {
                            continue;
                        }
                        *g.m.s.time.entry(id).or_default() += d;
                        let ahead = g.m.eng.w.node(id).is_some_and(|n| g.m.eng.w.work.get(&n.unit).is_some_and(|m| m.below(&n.pos).is_some()));
                        if let Err(e) = g.m.one(&snap, usize::from(ahead), id, o, &mut Sequential) {
                            g.m.err = Some(e);
                        }
                        g.merged += 1;
                    }
                    if g.m.err.is_none()
                        && let Err(e) = g.m.eng.check_deadline(g.m.s, g.m.rep, deadline)
                    {
                        g.m.err = Some(e);
                    }
                    if g.stop || g.m.err.is_some() {
                        g.stop = true;
                        cv.notify_all();
                        break;
                    }
                    // The next ready node, unless the epoch is over.
                    let job = if g.merged < budget { g.pick(window, predict) } else { None };
                    if let Some((id, resume)) = job {
                        let careful = g.m.s.refutes.contains_key(&id);
                        let fueled = g.fueled.remove(&id);
                        g.inflight.insert(id);
                        let snap = g.m.eng.w.clone();
                        let core = std::sync::Arc::clone(&g.m.eng.core);
                        drop(g);
                        #[cfg(not(target_arch = "wasm32"))]
                        let t = std::time::Instant::now();
                        let o = super::core::prof::time(0, || run_job(&core, &snap, id, resume, predict && !careful, careful, fueled.then_some(fuel)));
                        #[cfg(not(target_arch = "wasm32"))]
                        let d = t.elapsed();
                        #[cfg(target_arch = "wasm32")]
                        let d = std::time::Duration::ZERO;
                        g = super::core::lock(&flow);
                        g.inflight.remove(&id);
                        g.done.push((id, snap, o, d));
                        cv.notify_all();
                        continue;
                    }
                    if g.inflight.is_empty() && g.done.is_empty() {
                        // (Nothing ready, nothing running: the epoch ends.)
                        g.stop = true;
                        cv.notify_all();
                        break;
                    }
                    g = cv.wait(g).unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                crate::api::RunResult(())
            };
            let mut out = Vec::with_capacity(par);
            exec.execute(par, &run, &mut out);
            let mut f = flow.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
            // (Jobs an executor dropped: their nodes run again.)
            for (id, resume) in f.buf.drain(..) {
                if let Some(r) = resume {
                    f.m.eng.kept.insert(id, r);
                }
                f.m.eng.enqueue(f.m.s, id);
            }
            let (applied, parked, merged, err) = (f.m.applied, f.m.parked, f.merged, f.m.err.take());
            drop(f);
            if let Some(e) = err {
                return Err(e);
            }
            rep.steps += 1;
            width(rep, applied);
            let _ = merged;
            idle = if applied == 0 && parked == 0 && !s.queue.is_empty() { idle + 1 } else { 0 };
            if idle > self.limits.idle_rounds {
                let waiters = s
                    .queue
                    .iter()
                    .map(|k| crate::api::Waiter {
                        node: NodeId(k.2),
                        on: crate::api::WaitOn::Queued,
                    })
                    .collect();
                return Err(EvalError::NoProgress {
                    rounds: idle,
                    waiters,
                    report: Box::new(rep.clone()),
                });
            }
            if self.step_absorb(s, rep, src, per_round, rounds)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// A batch from the ready window: the first of the queue in text order
    /// whose read hints do not wait on a node in work (learned waits), at
    /// least the first, as the scheduler picks among them; the others go
    /// back to the queue. `skip`: nodes running (they stay queued).
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn select_batch(
        &mut self,
        s: &mut Sched,
        rep: &mut Report,
        ran: &mut FxHashSet<Id>,
        cut: &mut FxHashSet<Id>,
        hs: &mut Heights,
        window: usize,
        predict: bool,
        sched: &mut dyn Scheduler,
        skip: &FxHashSet<Id>,
    ) -> Result<Option<(Vec<Id>, FxHashMap<Id, Option<(Box<dyn std::any::Any + Send>, Vec<Read>)>>, FxHashSet<Id>)>, EvalError> {
        // The ready nodes: the first of the queue in text order whose
        // read hints do not wait on a node in work (learned waits); at
        // least the first.
        let mut ready: Vec<(u32, Pos, Id)> = Vec::new();
        let mut resumes: FxHashMap<Id, Option<(Box<dyn std::any::Any + Send>, Vec<Read>)>> = FxHashMap::default();
        let scan: Vec<(u32, Pos, Id)> = s.queue.iter().take(window * 4).copied().collect();
        for k in scan {
            if ready.len() >= window {
                break;
            }
            if skip.contains(&k.2) {
                continue;
            }
            // (The first always goes, unless nodes are running: they
            // finish and let it go.)
            let first = ready.is_empty() && skip.is_empty();
            if !first && !self.w.ready_by_hints(&self.core, k.2, predict) {
                continue;
            }
            // A node refuted this evaluation runs in text order: when
            // nothing before it in its unit is in work (or it is first).
            if !first && s.refutes.contains_key(&k.2) && self.w.work.get(&k.0).is_some_and(|m| m.below(&k.1).is_some()) {
                continue;
            }
            s.queue.remove(&k);
            if let Some(r) = self.prepare(s, rep, k) {
                if r.is_none()
                    && let Some(run) = self.reuse(k.2)
                {
                    self.unrun(rep, k.2);
                    self.apply(s, rep, ran, cut, hs, k.2, Outcome::Done(run), true)?;
                    continue;
                }
                resumes.insert(k.2, r);
                ready.push(k);
            }
        }
        if ready.is_empty() {
            return Ok(None);
        }
        let ids: Vec<NodeId> = ready.iter().map(|k| NodeId(k.2)).collect();
        let mut batch: Vec<crate::api::Job> = Vec::new();
        sched.select(&ids, &[], &mut batch);
        let mut chosen: Vec<Id> = Vec::new();
        let mut fueled: FxHashSet<Id> = FxHashSet::default();
        for j in &batch {
            if resumes.contains_key(&j.node.0) && !chosen.contains(&j.node.0) {
                chosen.push(j.node.0);
                // A speculative job runs on fuel (once out, not again).
                if j.speculative && !s.no_fuel.contains(&j.node.0) {
                    fueled.insert(j.node.0);
                }
            }
        }
        if chosen.is_empty() {
            chosen.push(ready[0].2);
        }
        // The rest go back to the queue (their kept state with them).
        for k in &ready {
            if !chosen.contains(&k.2) {
                // (Counted when prepared: it does not run.)
                rep.runs -= 1;
                if let Some(n) = self.w.node(k.2) {
                    let name = self.core.op_name(n.text.op);
                    if let Some(x) = rep.runs_by_op.iter_mut().find(|x| x.0 == name) {
                        x.1 -= 1;
                    }
                }
                if let Some(Some(r)) = resumes.remove(&k.2) {
                    self.kept.insert(k.2, r);
                }
                self.enqueue(s, k.2);
            }
        }
        Ok(Some((chosen, resumes, fueled)))
    }

    /// Node `id`'s result in the round just finished, when every read it
    /// made still holds here (strictly): it is this round's too.
    fn reuse(&self, id: Id) -> Option<Run> {
        let prev = self.ev_prev.as_ref()?;
        let pn = prev.node(id).filter(|n| n.st == St::Done)?;
        let cn = self.w.node(id)?;
        if !std::sync::Arc::ptr_eq(&pn.text, &cn.text) || self.w.units.get(&cn.unit).is_some_and(|un| un.app == id) {
            return None;
        }
        let res = pn.res.as_ref()?;
        if !res.reads.iter().all(|r| super::build::holds_with(&self.core, &self.w, id, r, true)) {
            return None;
        }
        let spec = |x: Id| -> Option<Spec> {
            let n = prev.node(x)?;
            Some(Spec {
                op: n.text.op,
                args: n.text.args.clone(),
                place: n.place,
            })
        };
        let kids: Vec<Spec> = res.kids.iter().map(|&x| spec(x)).collect::<Option<_>>()?;
        let conts: Vec<Spec> = res.conts.iter().map(|&x| spec(x)).collect::<Option<_>>()?;
        let mut r = (**res).clone();
        r.kids.clear();
        r.conts.clear();
        Some(Run {
            res: r,
            kids,
            conts,
            kept: None,
            apps: Vec::new(),
            env: Vec::new(),
            pset_wait: false,
            predicted: 0,
            guessed: false,
            fuel_out: false,
            range_wait: None,
            narrowed: None,
        })
    }

    /// A prepared node took its result without running: not a run.
    fn unrun(&self, rep: &mut Report, id: Id) {
        rep.runs -= 1;
        if let Some(n) = self.w.node(id) {
            let name = self.core.op_name(n.text.op);
            if let Some(x) = rep.runs_by_op.iter_mut().find(|x| x.0 == name) {
                x.1 -= 1;
            }
        }
    }

    /// The limits on runs and time: past one, the evaluation ends naming
    /// the nodes still to run.
    fn check_deadline(&self, s: &Sched, rep: &Report, deadline: Option<std::time::Instant>) -> Result<(), EvalError> {
        if !(self.limits.total_runs.is_some_and(|t| rep.runs >= t) || deadline.is_some_and(|d| std::time::Instant::now() >= d)) {
            return Ok(());
        }
        let mut waiters: Vec<crate::api::Waiter> = s
            .queue
            .iter()
            .map(|k| crate::api::Waiter {
                node: NodeId(k.2),
                on: crate::api::WaitOn::Queued,
            })
            .collect();
        waiters.extend(s.parked.iter().map(|(&id, w)| crate::api::Waiter {
            node: NodeId(id),
            on: match w {
                Wait::Node(x, f) => crate::api::WaitOn::Node(NodeId(*x), *f),
                Wait::Env(f) => crate::api::WaitOn::Environment(*f),
                Wait::Child(_) => crate::api::WaitOn::Queued,
            },
        }));
        Err(EvalError::Deadline {
            runs: rep.runs,
            waiters,
            report: Box::new(rep.clone()),
        })
    }

    /// Whether queued node `k` is to run now (and with what kept state): it
    /// is dirty where it was queued, and no pending claim holds it back.
    #[allow(clippy::type_complexity)]
    fn prepare(&mut self, s: &mut Sched, rep: &mut Report, k: (u32, Pos, Id)) -> Option<Option<(Box<dyn std::any::Any + Send>, Vec<Read>)>> {
        let id = k.2;
        s.queued.remove(&id);
        let n = self.w.node(id)?;
        if n.st != St::Dirty {
            return None;
        }
        if n.pos != k.1 || n.unit != k.0 {
            // Moved since queued.
            self.enqueue(s, id);
            return None;
        }
        // Stopped on a range: parked again, without a run, while the range
        // still has a node in work.
        if let Some((f, from, to)) = s.range_wait.remove(&id)
            && let Some((u, lo, hi)) = self.w.window(id, from, to)
            && let Some(x) = self.w.range_blocker(id, f, u, lo, hi)
        {
            s.range_wait.insert(id, (f, from, to));
            s.parked.insert(id, Wait::Node(x, Some(f)));
            s.waiters.entry(x).or_default().push(id);
            return None;
        }
        // Held back by an earlier claimer whose claim is pending.
        if let Some(c) = super::core::prof::time(5, || self.w.held_by(&self.core, id)) {
            s.parked.insert(id, Wait::Node(c, None));
            s.waiters.entry(c).or_default().push(id);
            rep.stops += 1;
            return None;
        }
        rep.runs += 1;
        if let Some(n) = self.w.node(id) {
            bump(&mut rep.runs_by_op, self.core.op_name(n.text.op));
        }
        // A stopped run's kept state: resumed while its reads hold.
        Some(match self.kept.remove(&id) {
            Some((tok, reads)) => match reads.iter().find(|r| !super::build::holds(&self.core, &self.w, id, r)) {
                None => {
                    rep.resumed += 1;
                    Some((tok, reads))
                }
                Some(r) => {
                    rep.restarted += 1;
                    bump(&mut rep.restarted_by, read_kind(r));
                    None
                }
            },
            None => None,
        })
    }

    /// A step's end: edits that arrived come in (in per-round rounds, the
    /// pass starts again: `true`); marked nodes the evaluation passed are
    /// reported.
    fn step_absorb(&mut self, s: &mut Sched, rep: &mut Report, src: &mut dyn EditSource, per_round: bool, rounds: u64) -> Result<bool, EvalError> {
        let before = self.edits;
        if let Some(v) = src.absorb(self) {
            self.version = v;
            rep.edits_applied.push((v, rounds));
        }
        if self.edits != before {
            if let Some(e) = self.hook_error() {
                return Err(e);
            }
            if per_round {
                return Ok(true);
            }
            for x in std::mem::take(&mut self.pending) {
                self.enqueue(s, x);
            }
        }
        if !per_round {
            self.report_settled(src, rep, false);
        }
        Ok(false)
    }

    /// Applies a run's outcome: a commit, a stop (parked on what it waits
    /// for), or the evaluation's error.
    #[allow(clippy::too_many_arguments)]
    fn apply(
        &mut self,
        s: &mut Sched,
        rep: &mut Report,
        ran: &mut FxHashSet<Id>,
        cut: &mut FxHashSet<Id>,
        hs: &mut Heights,
        id: Id,
        outcome: Outcome,
        fresh: bool,
    ) -> Result<(), EvalError> {
            match outcome {
                Outcome::Failed(mut e) => {
                    if let EvalError::NoProgress { rounds, .. } = &mut e {
                        *rounds = self.limits.idle_rounds.saturating_add(1);
                    }
                    return Err(e);
                }
                Outcome::Done(r) => {
                    if let Some(e) = self.runaway(id, &r.kids, &r.conts) {
                        return Err(e);
                    }
                    // (Applied runs count; a stop applies nothing.)
                    let why = super::core::lock(&self.core.causes).get(&id).copied();
                    if let Some(w) = why {
                        let q = s.why.entry(id).or_default();
                        q.push_back(cause_of(w));
                        if q.len() > 8 {
                            q.pop_front();
                        }
                    }
                    let c = s.runs.entry(id).or_insert(0);
                    *c += 1;
                    if *c > self.limits.runs_per_node {
                        return Err(EvalError::RunawayNode {
                            node: NodeId(id),
                            runs: *c,
                            causes: s.why.remove(&id).map(Vec::from).unwrap_or_default(),
                        });
                    }
                    let was_guess = if r.guessed { !s.guessed.insert(id) } else { s.guessed.remove(&id) };
                    if !ran.insert(id) && was_guess {
                        // Committed again: its earlier result rested on a
                        // guess that broke (a round's loop-back aside; a
                        // rerun for a committed value's change is none).
                        match super::core::lock(&self.core.causes).get(&id).copied() {
                            Some(crate::api::Rerun::Changed { field, .. } | crate::api::Rerun::Rebind { field }) => {
                                rep.mispredictions += 1;
                                *s.refutes.entry(id).or_default() += 1;
                                bump_field(&mut rep.refuted_by_field, field);
                            }
                            Some(crate::api::Rerun::Loop { .. }) => {}
                            _ => {
                                rep.mispredictions += 1;
                                *s.refutes.entry(id).or_default() += 1;
                            }
                        }
                    }
                    if hs.on {
                        self.height(hs, id, &r.res);
                    }
                    let dbg = super::core::debug().then(|| format!("{:?} out {}", r.res.reads, self.core.values.render(r.res.out)));
                    let w = std::mem::take(&mut self.w);
                    let mut b = Builder::new(&self.core, w);
                    b.app_env(id, &r.env);
                    b.defer = self.defer.take();
                    let changed = super::core::prof::time(9, || b.commit(id, r.res, &r.kids, &r.conts));
                    let mut m = b.finish();
                    self.defer = m.defer.take();
                    if let Some(d) = dbg {
                        eprintln!("run {id} reads {d} dirty {:?} woken {:?}", m.dirty, m.woken);
                    }
                    m.counts.add_to(rep);
                    let (dirty, created, freed, woken) = (m.dirty, m.created, m.freed, m.woken);
                    self.rebinds += m.rebinds;
                    self.w = m.w;
                    if changed {
                        cut.remove(&id);
                    } else {
                        cut.insert(id);
                    }
                    self.created += created;
                    self.freed += freed.len() as u64;
                    for x in &freed {
                        self.kept.remove(x);
                        s.parked.remove(x);
                        s.env.remove(x);
                    }
                    for x in dirty {
                        self.enqueue(s, x);
                    }
                    for x in woken {
                        self.wake(s, x);
                    }
                    if let Some(e) = self.hook_error() {
                        return Err(e);
                    }
                }
                Outcome::Stopped(r, wait) => {
                    rep.stops += 1;
                    if let Some(rw) = r.range_wait {
                        s.range_wait.insert(id, rw);
                    }
                    // Its narrowed pending set: the readers waiting on it
                    // look again.
                    if let Some(m) = r.narrowed
                        && let Some(mut n) = self.w.node(id).cloned()
                    {
                        n.narrowed = Some(std::sync::Arc::new(m));
                        self.w.put(id, n);
                        self.wake(s, id);
                    }
                    if r.pset_wait {
                        rep.pending_sets += 1;
                        rep.pending_waits += 1;
                    }
                    if let Some(n) = self.w.node(id) {
                        bump(&mut rep.waits_by_op, self.core.op_name(n.text.op));
                    }
                    // A stopped run's claim: siblings past it go on.
                    if let Some(n) = r.res.claim
                        && let Some(mut st) = self.w.claims.get(&id).cloned()
                        && st.decl == crate::api::Claim::Pending
                        && st.early != Some(n)
                    {
                        st.early = Some(n);
                        self.w.claims.insert(id, st, &NoMeasure);
                        self.w.refresh_holder(id);
                        self.w.refresh_claim_at(id);
                        self.wake(s, id);
                    }
                    if super::core::debug() {
                        eprintln!("stop {id} on {wait:?} reads {:?} node {:?}", r.res.reads, self.w.node(match wait { Wait::Node(x, _) => x, _ => id }));
                    }
                    if let Some(k) = r.kept {
                        self.kept.insert(id, (k, r.res.reads.clone()));
                    }
                    // Applications asked for: made now; an application's
                    // environment set before its body runs.
                    if !r.apps.is_empty() || !r.env.is_empty() {
                        let w = std::mem::take(&mut self.w);
                        let mut b = Builder::new(&self.core, w);
                        b.app_env(id, &r.env);
                        for (app, op, args) in &r.apps {
                            b.create_app(*app, *op, args.clone());
                        }
                        let m = b.finish();
                        self.w = m.w;
                        self.created += m.created;
                        for x in m.dirty {
                            self.enqueue(s, x);
                        }
                        for x in m.woken {
                            self.wake(s, x);
                        }
                    }
                    let subject = match &wait {
                        Wait::Child(k) => {
                            if let Some(e) = self.runaway(id, &r.kids, &r.conts) {
                                return Err(e);
                            }
                            let w = std::mem::take(&mut self.w);
                            let mut b = Builder::new(&self.core, w);
                            b.commit_unfolding(id, &r.kids, &r.conts);
                            let m = b.finish();
                            let (dirty, created, freed, woken) = (m.dirty, m.created, m.freed, m.woken);
                            self.rebinds += m.rebinds;
                            self.w = m.w;
                            self.created += created;
                            self.freed += freed.len() as u64;
                            for x in dirty {
                                self.enqueue(s, x);
                            }
                            for x in woken {
                                self.wake(s, x);
                            }
                            self.w
                                .node(id)
                                .and_then(|n| n.res.as_ref())
                                .and_then(|r| r.kids.get(*k).copied())
                        }
                        Wait::Node(x, _) => Some(*x),
                        Wait::Env(f) => {
                            s.env.insert(id, *f);
                            None
                        }
                    };
                    if let Wait::Node(x, f) = &wait {
                        hs.woke.insert(id, (*x, *f));
                    }
                    s.parked.insert(id, wait);
                    if let Some(x) = subject {
                        // Done already (a race in this world's making):
                        // run again at once.
                        if self.w.node(x).is_none_or(|n| n.st == St::Done || matches!(n.st, St::Inert(_))) {
                            // (Run on this very world, counted as a run: a
                            // read that keeps stopping on a finished node is
                            // a runaway, not a hang. On a step's snapshot it
                            // finished meanwhile: progress.)
                            let c = s.runs.entry(id).or_insert(0);
                            *c += u32::from(fresh);
                            if *c > self.limits.runs_per_node {
                                if super::core::debug() {
                                    eprintln!("stops on finished {x}: {:?}", self.w.node(x).map(|n| (n.st, n.unit, n.pos)));
                                }
                                return Err(EvalError::RunawayNode {
                                    node: NodeId(id),
                                    runs: *c,
                                    causes: Vec::new(),
                                });
                            }
                            s.parked.remove(&id);
                            self.enqueue(s, id);
                        } else {
                            s.waiters.entry(x).or_default().push(id);
                        }
                    }
                }
            }
        Ok(())
    }

    /// The unfolding limits: a run of `id` placing `kids` and `conts`.
    fn runaway(&self, id: Id, kids: &[Spec], conts: &[Spec]) -> Option<EvalError> {
        let created = u32::try_from(kids.len() + conts.len()).unwrap_or(u32::MAX);
        let depth = self.w.node(id).map_or(0, |n| n.depth);
        if !((!kids.is_empty() && depth + 1 > self.limits.unfold_depth) || created > self.limits.children_per_node) {
            return None;
        }
        let keep = created.min(self.limits.children_per_node.saturating_add(1)).min(8) as usize;
        let mut last: Vec<(String, String)> = kids
            .iter()
            .chain(conts)
            .rev()
            .take(keep)
            .filter(|sp| !matches!(sp.place, super::world::Place::Marker(_)))
            .map(|sp| {
                let args = sp.args.iter().map(|&a| self.core.values.render(a)).collect::<Vec<_>>().join(" ");
                (self.core.op_name(sp.op).to_string(), args)
            })
            .collect();
        last.reverse();
        Some(EvalError::RunawayCreation {
            node: NodeId(id),
            created,
            depth: depth + 1,
            last,
        })
    }

    /// Node `id`'s run committed `res`: its height, one above the highest
    /// run of this evaluation it read from, waited on or was made by.
    fn height(&self, hs: &mut Heights, id: Id, res: &Res) {
        use crate::api::Cause;
        let mut best: Option<(u32, Id, Cause)> = None;
        let mut offer = |hs: &Heights, x: Id, c: Cause| {
            if let Some(&(h, _, _)) = hs.h.get(&x)
                && best.as_ref().is_none_or(|b| h > b.0)
            {
                best = Some((h, x, c));
            }
        };
        for r in &res.reads {
            match r {
                Read::Use { f, site, .. } if *site != super::world::NONE => {
                    offer(hs, *site, Cause::Changed {
                        field: *f,
                        site: Some(NodeId(*site)),
                    });
                }
                Read::Out { node, .. } => offer(hs, *node, Cause::Output { of: NodeId(*node) }),
                _ => {}
            }
        }
        if let Some(&(x, f)) = hs.woke.get(&id) {
            // (Waiting on a pending set declared by prefixes: the one the
            // field matched.)
            let prefix = f.and_then(|f| {
                let ps = match self.w.node(x).map(super::world::Node::may) {
                    Some(super::world::May::Prefixes(ps)) => ps.clone(),
                    _ => return None,
                };
                self.core.values.with_str(f.0, |name| ps.iter().find(|p| name.starts_with(p.as_str())).cloned()).flatten()
            });
            offer(hs, x, Cause::Woken {
                by: NodeId(x),
                field: f,
                prefix,
            });
        }
        let parent = self.w.node(id).map_or(super::world::NONE, |n| n.parent);
        let created = Cause::Created {
            by: (parent != super::world::NONE).then_some(NodeId(parent)),
            from_hint: None,
        };
        if parent != super::world::NONE {
            offer(hs, parent, created.clone());
        }
        // The definers of its declared reads (hints), as its start.
        if let Some(n) = self.w.node(id) {
            let o = &self.core.ops[n.text.op.0 as usize];
            let hints = self.core.hook(Some(id), n.text.op, "reads", || None, || o.reads(&n.text.args, &self.core.values));
            if let (Some(fs), Some((u, pos, fr))) = (hints, self.w.frame_at(id, false)) {
                for f in fs {
                    if let Some((_, e)) = self.w.reaching(u, f, pos, fr, id) {
                        offer(hs, e.node, Cause::Created {
                            by: (parent != super::world::NONE).then_some(NodeId(parent)),
                            from_hint: Some((f, NodeId(e.node))),
                        });
                    }
                }
            }
        }
        let v = match best {
            Some((h, x, c)) => (h + 1, Some(x), c),
            None => (0, None, created),
        };
        hs.h.insert(id, v);
    }

    /// At quiescence, the done readers that stood on a guess past a node
    /// still in work (stuck) run again; whether any did.
    fn unguess(&mut self, s: &mut Sched) -> bool {
        let mut from: FxHashMap<u32, Pos> = FxHashMap::default();
        for &id in s.parked.keys() {
            if let Some(n) = self.w.node(id).filter(|n| super::world::in_work(n.st)) {
                let e = from.entry(n.unit).or_insert(n.pos);
                *e = (*e).min(n.pos);
            }
        }
        if from.is_empty() {
            return false;
        }
        let w = std::mem::take(&mut self.w);
        let mut b = Builder::new(&self.core, w);
        for (&u, &p) in &from {
            b.recheck_strict(u, p);
        }
        let m = b.finish();
        self.w = m.w;
        let any = !m.dirty.is_empty();
        for id in m.dirty {
            self.enqueue(s, id);
        }
        any
    }

    /// The parked nodes whose wait ends, through the nodes they wait on,
    /// at an environment wait (a stall the client resolves, not a lack of
    /// progress).
    /// The nodes the waiters wait on that are not waiting themselves, each
    /// with its state.
    fn stuck_subjects(&self, s: &Sched, stuck: &[crate::api::Waiter]) -> Vec<String> {
        let waiting: FxHashSet<NodeId> = stuck.iter().map(|w| w.node).collect();
        let mut subjects: Vec<NodeId> = stuck
            .iter()
            .filter_map(|w| match w.on {
                crate::api::WaitOn::Node(x, _) if !waiting.contains(&x) => Some(x),
                _ => None,
            })
            .collect();
        subjects.sort_unstable();
        subjects.dedup();
        subjects
            .into_iter()
            .map(|x| match self.w.node(x.0) {
                None => format!("{}: gone", x.0),
                Some(n) => format!(
                    "{}: {:?}, result {}, op {} {}, {:?} by {}, parked {:?}, queued {}, waiters {}",
                    x.0,
                    n.st,
                    n.res.is_some(),
                    self.core.op_name(n.text.op),
                    n.text.args.iter().take(2).map(|&a| self.core.values.render(a)).collect::<Vec<_>>().join(" ").chars().take(60).collect::<String>(),
                    n.place,
                    n.parent,
                    s.parked.get(&x.0),
                    s.queued.contains(&x.0),
                    s.waiters.get(&x.0).map_or(0, Vec::len),
                ),
            })
            .collect()
    }

    fn env_blocked(&self, s: &Sched) -> FxHashSet<Id> {
        let mut out: FxHashSet<Id> = s.env.keys().copied().collect();
        out.extend(s.parked.iter().filter(|(_, w)| matches!(w, Wait::Env(_))).map(|(&id, _)| id));
        let mut todo: Vec<Id> = out.iter().copied().collect();
        while let Some(x) = todo.pop() {
            for &y in s.waiters.get(&x).map_or(&[][..], |v| &v[..]) {
                if s.parked.contains_key(&y) && out.insert(y) {
                    todo.push(y);
                }
            }
        }
        out
    }

    /// Reports the marked nodes that are final (done, nothing before them
    /// in their unit in work; with loop-backs, only at the end) and were
    /// not reported at this version with this output.
    fn report_settled(&mut self, src: &mut dyn EditSource, rep: &mut Report, end: bool) {
        if (self.marked.is_empty() && self.w.op_marked.is_empty()) || (!end && !self.core.loops.is_empty()) {
            return;
        }
        let mut items: Vec<(Pos, crate::api::Settled)> = Vec::new();
        let mut marked: Vec<Id> = self.marked.iter().copied().chain(self.w.op_marked.iter().map(|(&x, ())| x)).collect();
        marked.sort_unstable();
        marked.dedup();
        for id in marked {
            let Some(n) = self.w.node(id) else { continue };
            if n.st != St::Done {
                continue;
            }
            if self.w.work.get(&n.unit).is_some_and(|m| m.below(&n.pos).is_some()) {
                continue;
            }
            let out = n.out().unwrap_or(ValueId::UNIT);
            match self.reported.get(&id) {
                Some(&(v, o)) if v == self.version && o == out => continue,
                Some(&(v, _)) if v == self.version => rep.resettled += 1,
                _ => {}
            }
            self.reported.insert(id, (self.version, out));
            items.push((
                n.pos,
                crate::api::Settled {
                    node: NodeId(id),
                    version: self.version,
                    output: out,
                },
            ));
        }
        if items.is_empty() {
            return;
        }
        items.sort_by_key(|i| i.0);
        let items: Vec<crate::api::Settled> = items.into_iter().map(|i| i.1).collect();
        rep.settled += items.len() as u64;
        if !src.settled(self, &items) {
            self.settled_out.extend(items);
        }
    }

    /// Requeues the nodes waiting on `x` (it committed or went).
    fn wake(&self, s: &mut Sched, x: Id) {
        if let Some(v) = s.waiters.remove(&x) {
            for id in v {
                s.parked.remove(&id);
                self.enqueue(s, id);
            }
        }
    }


    /// A cycle among parked nodes through outputs, if any.
    fn cycle(&self, s: &Sched) -> Option<Vec<Id>> {
        // 1: on the current path (at that index), 2: walked, no cycle.
        let mut seen: FxHashMap<Id, (u8, usize)> = FxHashMap::default();
        for &start in s.parked.keys() {
            // From a node outside the applications (an application nothing
            // outside it waits on is no error).
            if seen.contains_key(&start)
                || self.w.node(start).is_none_or(|n| self.w.units.get(&n.unit).is_some_and(|un| un.app != super::world::NONE))
            {
                continue;
            }
            let mut path: Vec<Id> = Vec::new();
            let mut x = start;
            loop {
                match seen.get(&x) {
                    Some(&(1, i)) => return Some(path[i..].to_vec()),
                    Some(_) => break,
                    None => {}
                }
                seen.insert(x, (1, path.len()));
                path.push(x);
                match s.parked.get(&x) {
                    Some(Wait::Node(y, _)) => x = *y,
                    Some(Wait::Child(k)) => {
                        let Some(c) = self.w.node(x).and_then(|n| n.res.as_ref()).and_then(|r| r.kids.get(*k).copied()) else {
                            break;
                        };
                        x = c;
                    }
                    _ => break,
                }
            }
            for p in path {
                seen.insert(p, (2, 0));
            }
        }
        None
    }


    /// A round ended: its runs, and (from its old world) the results it
    /// reused.
    fn round_done(&self, rep: &mut Report, round_runs: &mut u64, from_old: bool) {
        let runs = rep.runs - *round_runs;
        *round_runs = rep.runs;
        let memo_hits = if from_old { (self.w.nodes.len() as u64).saturating_sub(runs) } else { 0 };
        rep.round_times.push(crate::api::RoundTimes {
            runs,
            memo_hits,
            ..Default::default()
        });
    }

    fn publications(&self) -> Result<Vec<(Field, ValueId)>, EvalError> {
        publications_of(&self.core, &self.w)
    }

    /// Sets the current world's loop-back values; the readers of those that
    /// moved run again. Whether any moved.
    fn set_loops(&mut self, s: &mut Sched, vals: &[(Field, ValueId)]) -> bool {
        let mut moved = Vec::new();
        for &(f, x) in vals {
            if self.w.loops.get(&f) != Some(&x) {
                self.w.loops.insert(f, x, &NoMeasure);
                moved.push(f);
            }
        }
        if moved.is_empty() {
            return false;
        }
        let readers = self.loop_readers(&moved);
        let w = std::mem::take(&mut self.w);
        let mut b = Builder::new(&self.core, w);
        for &id in &readers {
            b.make_dirty_for(id, crate::api::Rerun::Loop { field: moved[0] });
        }
        let m = b.finish();
        self.w = m.w;
        for id in m.dirty {
            self.enqueue(s, id);
        }
        true
    }

    /// Queues the current world's nodes in work (`dirty` first), its
    /// claims settled.
    fn start_world(&mut self, s: &mut Sched, dirty: &[Id]) {
        let w = std::mem::take(&mut self.w);
        let m = Builder::new(&self.core, w).finish();
        self.w = m.w;
        for id in m.dirty {
            self.enqueue(s, id);
        }
        for x in m.woken {
            self.wake(s, x);
        }
        for &id in dirty {
            self.enqueue(s, id);
        }
        let left: Vec<Id> = self.w.nodes.iter().filter(|(_, n)| n.st == St::Dirty).map(|(&id, _)| id).collect();
        for id in left {
            self.enqueue(s, id);
        }
    }

    /// The committed readers of loop-back fields `fs`.
    fn loop_readers(&self, fs: &[Field]) -> Vec<Id> {
        let set: FxHashSet<Field> = fs.iter().copied().collect();
        self.w
            .nodes
            .iter()
            .filter(|(_, n)| {
                n.st == St::Done
                    && n.res
                        .as_ref()
                        .is_some_and(|r| r.reads.iter().any(|rd| matches!(rd, Read::Loop { f, .. } if set.contains(f))))
            })
            .map(|(&id, _)| id)
            .collect()
    }

    fn dirty_all(&mut self, s: &mut Sched, ids: &[Id]) {
        let w = std::mem::take(&mut self.w);
        let mut b = Builder::new(&self.core, w);
        for &id in ids {
            b.make_dirty(id);
        }
        let m = b.finish();
        self.w = m.w;
        for id in m.dirty {
            self.enqueue(s, id);
        }
    }
}

/// Adds one to `name`'s count in `list`.
fn bump(list: &mut Vec<(&'static str, u64)>, name: &'static str) {
    match list.iter_mut().find(|x| x.0 == name) {
        Some(x) => x.1 += 1,
        None => list.push((name, 1)),
    }
}

/// The critical chain: from the highest run down its links.
fn chain_of(hs: &Heights) -> Vec<(NodeId, u32, crate::api::Cause)> {
    let Some((&top, _)) = hs.h.iter().max_by_key(|(id, v)| (v.0, std::cmp::Reverse(**id))) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut x = Some(top);
    while let Some(id) = x {
        let Some((h, next, c)) = hs.h.get(&id) else { break };
        out.push((NodeId(id), *h, c.clone()));
        x = *next;
        if out.len() > hs.h.len() {
            break;
        }
    }
    out
}

/// A read's kind, as `Report::restarted_by` names why a kept state went.
fn read_kind(r: &Read) -> &'static str {
    match r {
        Read::Use { .. } => "use: changed",
        Read::Loop { .. } => "loop: changed",
        Read::Env { .. } => "env: changed",
        Read::Out { .. } => "output: changed",
        Read::Kind { .. } => "kind: changed",
        Read::IsLoop { .. } => "is_loopback: changed",
        Read::At { .. } => "position: gone",
        Read::Range { .. } => "range: changed",
        Read::Sib { .. } => "following: changed",
        Read::Claimed { .. } => "claimed: changed",
        Read::Prev { .. } => "preceding: changed",
        Read::Stream { .. } => "stream: changed",
        Read::Seq { .. } => "sequence: changed",
    }
}

/// The loop-back values world `w`'s publications give (every declared
/// field: unpublished, its initial value).
fn publications_of(core: &super::core::Core, w: &World) -> Result<Vec<(Field, ValueId)>, EvalError> {
    if core.loops.is_empty() {
        return Ok(Vec::new());
    }
    let mut pubs: FxHashMap<Field, Vec<ValueId>> = FxHashMap::default();
    for (_, n) in w.nodes.iter() {
        if n.st != St::Done {
            continue;
        }
        let Some(r) = &n.res else { continue };
        for &(f, x) in &r.pubs {
            pubs.entry(f).or_default().push(x);
        }
    }
    let mut fields: Vec<Field> = core.loops.keys().copied().collect();
    fields.sort_by_key(|f| f.0.raw());
    let mut out = Vec::new();
    for f in fields {
        let mut vals = pubs.remove(&f).unwrap_or_default();
        vals.sort_by_key(|v| v.raw());
        vals.dedup();
        if vals.len() > 1 {
            return Err(EvalError::ConflictingPublication { field: f });
        }
        out.push((f, vals.first().copied().unwrap_or(core.loops[&f])));
    }
    Ok(out)
}

/// The state the flow's workers share (under its mutex).
struct Flow<'a> {
    m: Merge<'a>,
    sched: crate::sync::SendMut<'a, dyn Scheduler + 'a>,
    /// Chosen, not yet running: each with its kept state.
    buf: std::collections::VecDeque<(Id, Option<(Box<dyn std::any::Any + Send>, Vec<Read>)>)>,
    fueled: FxHashSet<Id>,
    inflight: FxHashSet<Id>,
    /// Finished runs: the world each ran on, and what it gave.
    done: Vec<(Id, World, Outcome, std::time::Duration)>,
    merged: usize,
    stop: bool,
}

impl Flow<'_> {
    /// The next node to run: from the batch chosen last, else a new one.
    fn pick(&mut self, window: usize, predict: bool) -> Option<(Id, Option<(Box<dyn std::any::Any + Send>, Vec<Read>)>)> {
        // (A chosen node a merge freed, finished or queued again since is
        // dropped: queued, it is chosen again in its turn.)
        while let Some((id, r)) = self.buf.pop_front() {
            let live = self.m.eng.w.node(id).is_some_and(|n| n.st == St::Dirty);
            if live && !self.inflight.contains(&id) && !self.m.s.queued.contains(&id) {
                return Some((id, r));
            }
            // (Counted when prepared: it does not run.)
            self.m.rep.runs -= 1;
            if live && let Some(r) = r {
                self.m.eng.kept.insert(id, r);
            }
        }
        {
            let m = &mut self.m;
            let inflight = &self.inflight;
            let got = self.sched.with(|sched| m.eng.select_batch(m.s, m.rep, m.ran, m.cut, m.hs, window, predict, sched, inflight));
            match got {
                Err(e) => {
                    self.m.err = Some(e);
                    return None;
                }
                Ok(None) => return None,
                Ok(Some((mut chosen, mut resumes, fueled))) => {
                    chosen.sort_by_key(|&id| self.m.eng.w.node(id).map(|n| (n.unit, n.pos)));
                    self.fueled.extend(fueled);
                    for id in chosen {
                        let r = resumes.remove(&id).flatten();
                        self.buf.push_back((id, r));
                    }
                }
            }
        }
        self.buf.pop_front()
    }
}

/// A step's merge: its results applied in text order as they come.
struct Merge<'a> {
    eng: &'a mut Engine,
    s: &'a mut Sched,
    rep: &'a mut Report,
    ran: &'a mut FxHashSet<Id>,
    cut: &'a mut FxHashSet<Id>,
    hs: &'a mut Heights,
    /// The next result (by index, in text order) to merge.
    next: usize,
    applied: u64,
    parked: u64,
    err: Option<EvalError>,
}

type Resume = std::sync::Mutex<Option<(Box<dyn std::any::Any + Send>, Vec<Read>)>>;

impl Merge<'_> {
    /// Merges the results ready in order; `all`: the batch is over (a job
    /// without a result runs again).
    fn drain(&mut self, snap: &World, jobs: &[(Id, Resume)], slots: &[std::sync::Mutex<Option<(Outcome, std::time::Duration)>>], exec: &mut dyn Executor) {
        while self.next < jobs.len() {
            let i = self.next;
            let id = jobs[i].0;
            let got = super::core::lock(&slots[i]).take();
            let Some((o, d)) = got else {
                // The executor gave no result: it runs again.
                self.next += 1;
                self.eng.enqueue(self.s, id);
                continue;
            };
            self.next += 1;
            if self.err.is_some() {
                continue;
            }
            *self.s.time.entry(id).or_default() += d;
            if let Err(e) = self.one(snap, i, id, o, exec) {
                self.err = Some(e);
            }
        }
    }

    /// Makes the recorded field writes; the readers they re-ask are
    /// settled.
    fn flush(&mut self, exec: &mut dyn Executor) {
        let eng = &mut *self.eng;
        let Some(p) = eng.defer.as_mut() else { return };
        if p.is_empty() {
            return;
        }
        let mut w = std::mem::take(&mut eng.w);
        let re = super::core::prof::time(20, || p.flush(&eng.core, &mut w, exec));
        let mut b = Builder::new(&eng.core, w);
        b.recheck_all(re);
        let m = b.finish();
        m.counts.add_to(self.rep);
        eng.rebinds += m.rebinds;
        eng.w = m.w;
        for x in m.dirty {
            eng.enqueue(self.s, x);
        }
        for x in m.woken {
            eng.wake(self.s, x);
        }
    }

    /// Whether result `o` of `id` changes structure (or claims): its
    /// commit is made at once, after the recorded writes.
    fn structural(eng: &Engine, id: Id, o: &Outcome) -> bool {
        use super::core::prof::count;
        let Outcome::Done(r) = o else {
            count(11);
            return true;
        };
        let Some(n) = eng.w.node(id) else { return true };
        let old = n.res.as_deref();
        if !r.kids.is_empty() || !r.conts.is_empty() || old.is_some_and(|x| !x.kids.is_empty() || !x.conts.is_empty()) {
            count(7);
            return true;
        }
        // The brackets it has now against those the result gives it.
        let now = n.brackets();
        let then = match (r.res.effect, n.text.stat.effect) {
            (Some(e), _) | (None, Some(e)) => e.brackets(),
            (None, None) => crate::api::Brackets::default(),
        };
        if now != then || old.map_or(&[][..], |x| &x.kinds[..]) != &r.res.kinds[..] {
            count(8);
            return true;
        }
        // A claimer that claims nothing and holds nothing settles without
        // touching definitions (its claims' writes, if any, are recorded
        // too); one reading claimed siblings or streams does not.
        let held = eng.w.claims.get(&id).is_some_and(|st| !st.taken.is_empty());
        let sibs = r.res.reads.iter().any(|rd| matches!(rd, Read::Stream { .. } | Read::Claimed { .. } | Read::Sib { .. }));
        if r.res.claim.is_some_and(|k| k > 0) || held || sibs {
            count(9);
            return true;
        }
        if !r.env.is_empty() {
            count(10);
            return true;
        }
        count(6);
        false
    }

    /// Whether result `o` reads a field with writes recorded.
    fn reads_pending(eng: &Engine, o: &Outcome) -> bool {
        let Some(p) = eng.defer.as_ref().filter(|p| !p.is_empty()) else { return false };
        let reads = match o {
            Outcome::Done(r) | Outcome::Stopped(r, _) => &r.res.reads,
            Outcome::Failed(_) => return false,
        };
        reads.iter().any(|rd| match rd {
            Read::Use { f, .. } | Read::Range { f, .. } | Read::Seq { f, .. } => p.fields.contains(f),
            Read::Stream { fields, .. } => fields.iter().any(|(f, _)| p.fields.contains(f)),
            Read::Out { .. } | Read::At { .. } | Read::Kind { .. } | Read::Loop { .. } | Read::Env { .. } | Read::IsLoop { .. } => false,
            _ => true,
        })
    }

    fn one(&mut self, snap: &World, i: usize, id: Id, o: Outcome, exec: &mut dyn Executor) -> Result<(), EvalError> {
        let structural = Self::structural(self.eng, id, &o);
        if structural || Self::reads_pending(self.eng, &o) {
            self.flush(exec);
        }
        let saved = if structural { self.eng.defer.take() } else { None };
        let r = self.one_inner(snap, i, id, o);
        if structural {
            self.eng.defer = saved;
        }
        r
    }

    fn one_inner(&mut self, snap: &World, i: usize, id: Id, o: Outcome) -> Result<(), EvalError> {
        let (eng, s, rep) = (&mut *self.eng, &mut *self.s, &mut *self.rep);
        if !eng.w.node(id).is_some_and(|n| n.st == St::Dirty) {
            return Ok(());
        }
        // Re-texted by an earlier commit of this step (a placer's): the run
        // is of the old text.
        let same_text = match (snap.node(id), eng.w.node(id)) {
            (Some(a), Some(b)) => std::sync::Arc::ptr_eq(&a.text, &b.text),
            _ => false,
        };
        if !same_text {
            eng.enqueue(s, id);
            return Ok(());
        }
        if super::core::prof::on() {
            conflict_counts(snap, &eng.w, id, &o);
        }
        // (Behind an earlier job of its step: it ran on a snapshot that
        // job's commit may change.)
        let ahead = i > 0;
        rep.speculations += u64::from(ahead);
        let o = match o {
            Outcome::Stopped(r, _) if r.fuel_out => {
                rep.fuel_outs += 1;
                s.no_fuel.insert(id);
                if let Some(k) = r.kept {
                    eng.kept.insert(id, (k, r.res.reads.clone()));
                }
                eng.enqueue(s, id);
                return Ok(());
            }
            Outcome::Done(r) => match r.res.reads.iter().find(|rd| !super::build::holds(&eng.core, &eng.w, id, rd)) {
                Some(rd) => {
                    // A read an earlier commit of this step broke.
                    eng.core.cause(id, super::build::why_broken(&eng.core, &eng.w, id, rd));
                    *s.refutes.entry(id).or_default() += 1;
                    rep.mispredictions += 1;
                    rep.predictions_refuted += u64::from(r.predicted > 0);
                    if let Read::Use { f, .. } | Read::Range { f, .. } | Read::Seq { f, .. } = rd {
                        bump_field(&mut rep.refuted_by_field, *f);
                    }
                    eng.enqueue(s, id);
                    return Ok(());
                }
                None => {
                    rep.predictions_confirmed += u64::from(r.predicted > 0);
                    rep.confirmations += u64::from(ahead);
                    Outcome::Done(r)
                }
            },
            o => o,
        };
        let done = matches!(o, Outcome::Done(_));
        if done {
            self.applied += 1;
        }
        super::core::prof::time(1, || eng.apply(s, rep, self.ran, self.cut, self.hs, id, o, false))?;
        // A stop that parks its node (it leaves the queue) moves on.
        if !done && !s.queued.contains(&id) {
            self.parked += 1;
        }
        Ok(())
    }
}

/// Counts (CLEAN_PROF) whether a result read keys an earlier commit of its
/// step changed: data keys, or only its unit's shape.
fn conflict_counts(snap: &World, w: &World, id: Id, o: &Outcome) {
    use super::core::prof::count;
    use super::world::Key;
    let Outcome::Done(r) = o else { return };
    count(0);
    let Some(u) = snap.node(id).map(|n| n.unit) else { return };
    let moved = |k: Key| w.key_ver(&k) != snap.key_ver(&k);
    let unit_of = |x: Id| snap.node(x).map_or(u, |n| n.unit);
    let mut data = false;
    let mut shape = false;
    for rd in r.res.reads.iter() {
        match rd {
            Read::Use { f, at, .. } => {
                data |= moved(Key::Defs(unit_of(*at), *f));
                shape |= moved(Key::Shape(unit_of(*at)));
            }
            Read::Out { node, .. } => data |= moved(Key::Out(*node)),
            Read::Range { f, .. } | Read::Seq { f, .. } => {
                data |= moved(Key::Acc(u, *f)) || moved(Key::Defs(u, *f));
                shape |= moved(Key::Shape(u));
            }
            Read::Env { f, .. } => data |= moved(Key::Env(*f)),
            Read::Loop { f, .. } => data |= moved(Key::Loop(*f)),
            _ => shape |= moved(Key::Shape(u)),
        }
    }
    if data {
        count(2);
    } else if shape {
        count(3);
    } else {
        count(1);
    }
    if !r.kids.is_empty() || !r.conts.is_empty() {
        count(4);
    }
    if snap.node(id).is_some_and(|n| n.res.as_ref().map_or(n.text.stat.effect.is_none(), |x| x.effect != r.res.effect)) {
        count(5);
    }
}

/// Runs node `id` against world `w` (any thread).
fn run_job(
    core: &super::core::Core,
    w: &World,
    id: Id,
    resume: Option<(Box<dyn std::any::Any + Send>, Vec<Read>)>,
    predict: bool,
    careful: bool,
    fuel: Option<u64>,
) -> Outcome {
    let (kept, reads) = match resume {
        Some((t, r)) => (Some(t), r),
        None => (None, Vec::new()),
    };
    // Run ahead of a node before it still in work: what it reads may
    // change yet (a guess).
    let ahead = w.node(id).is_some_and(|n| w.work.get(&n.unit).is_some_and(|m| m.below(&n.pos).is_some()));
    let mut cx = Ctx::new(core, w, id, kept);
    cx.reads = reads;
    cx.predict = predict;
    cx.careful = careful;
    cx.fuel = fuel;
    let op = core.ops[cx.op.0 as usize].clone();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| op.run(&mut cx)));
    if let Some(e) = cx.error.take() {
        return Outcome::Failed(e);
    }
    let run = |cx: Ctx<'_>, out: ValueId| Run {
        res: Res {
            out,
            defs: cx.defs,
            contribs: cx.contribs,
            pubs: cx.pubs,
            effect: cx.effect,
            kinds: cx.kinds,
            kids: Vec::new(),
            conts: Vec::new(),
            reads: cx.reads,
            diag: cx.diag,
            claim: cx.claim,
        },
        kids: cx.kids,
        conts: cx.conts,
        kept: cx.kept,
        apps: cx.apps,
        env: cx.env,
        pset_wait: cx.pset_wait,
        predicted: cx.predicted,
        guessed: cx.guessed || cx.predicted > 0 || ahead,
        fuel_out: cx.fuel_out,
        range_wait: cx.range_wait,
        narrowed: cx.narrowed,
    };
    match r {
        Ok(Ok(out)) => Outcome::Done(run(cx, out)),
        Ok(Err(_)) => {
            let wait = cx.wait.take().unwrap_or(Wait::Node(id, None));
            Outcome::Stopped(run(cx, ValueId::UNIT), wait)
        }
        Err(p) => Outcome::Failed(EvalError::OpPanicked {
            node: NodeId(id),
            op: core.op_name(cx.op).to_string(),
            message: super::core::panic_message(&*p),
        }),
    }
}

/// Counts a step that applied `n` runs in the width histogram (bucket `i`:
/// widths up to `2^i`).
fn width(rep: &mut Report, n: u64) {
    if n == 0 {
        return;
    }
    let b = n.next_power_of_two().trailing_zeros() as usize;
    if rep.applied_widths.len() <= b {
        rep.applied_widths.resize(b + 1, 0);
    }
    rep.applied_widths[b] += 1;
}

/// Adds one to field `f`'s count in `list`.
fn bump_field(list: &mut Vec<(Field, u64)>, f: Field) {
    match list.iter_mut().find(|x| x.0 == f) {
        Some(x) => x.1 += 1,
        None => list.push((f, 1)),
    }
}
