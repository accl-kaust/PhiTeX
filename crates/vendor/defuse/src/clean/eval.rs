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
    range_wait: Option<(Field, Id, Id, bool)>,
    narrowed: Option<super::world::May>,
    /// Committed on a broken guessed fold, to run again: its folds waited
    /// for nothing.
    fold_redo: bool,
    /// When it started and ended (the critical chain's link times).
    span: Option<(std::time::Instant, std::time::Instant)>,
    /// It continued from its kept state (`Ctx::base`): its placements are
    /// its new ones, after those its earlier attempts made.
    base: Option<super::ctx::Base>,
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
    range_wait: FxHashMap<Id, (Field, Id, Id, bool)>,
    /// Nodes a batch left out by their read hints, parked on the node in
    /// work they wait on (not scanned again until it is done).
    hint_parked: FxHashMap<Id, Id>,
    /// Nodes waiting for their placer's placements to be made again, by
    /// placer.
    placed: FxHashMap<Id, Vec<Id>>,
    /// Refutations of guessed folds (`Monoid::speculate`) by node: the
    /// first two do not make it careful (the contributors' last
    /// contributions stand on the next run).
    fold_refutes: FxHashMap<Id, u32>,
    /// While a step's lane runs: the nodes its commits queued (its
    /// candidates to run next).
    lane_new: Option<Vec<Id>>,
    /// When each node was last queued, and that time as of its run's
    /// start (the critical chain's link times).
    ready_at: FxHashMap<Id, std::time::Instant>,
    ran_ready: FxHashMap<Id, std::time::Instant>,
    /// Placers whose kept state's reads were found to hold, with the world
    /// version they held at.
    kept_ok: FxHashMap<Id, u64>,
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
            fold_refutes: std::mem::take(&mut self.fold_refutes),
            ran_ready: std::mem::take(&mut self.ran_ready),
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

/// A run's height, the run it stood on, and why.
type Height = (u32, Option<Id>, crate::api::Cause);

/// Heights of the runs of one evaluation: each run's height and the link
/// to the run it stood on.
#[derive(Default)]
struct Heights {
    on: bool,
    h: FxHashMap<Id, Height>,
    /// The longest chain by run time ending at each run (ns, its own
    /// run time included), over the links `h` offers.
    t: FxHashMap<Id, u64>,
    /// Each node's entry (in `h`, `t`) as of the run that last changed its
    /// contributions: what a range read over it waited for (a rerun that
    /// contributes the same is not waited for).
    hc: FxHashMap<Id, (Height, u64, [u64; 4])>,
    /// What each parked node last waited on.
    woke: FxHashMap<Id, (Id, Option<Field>)>,
    /// The evaluation's start, and for each node's last committed run
    /// when it was queued, started, ended and committed (ns since).
    epoch: Option<std::time::Instant>,
    at: FxHashMap<Id, [u64; 4]>,
    /// The node `height` just filed in `hc` (its commit time goes there too).
    hc_new: Option<Id>,
}

impl Heights {
    /// Node `id`'s run committed now.
    fn committed(&mut self, id: Id) {
        let c = self.epoch.map_or(0, |e| super::core::ns_since(Some(e)));
        if let Some(a) = self.at.get_mut(&id) {
            a[3] = c;
        }
        if self.hc_new.take() == Some(id)
            && let Some(e) = self.hc.get_mut(&id)
        {
            e.2[3] = c;
        }
    }
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
        #[cfg(feature = "observe")]
        let mut rsrc = crate::record::RecSource { inner: src };
        #[cfg(feature = "observe")]
        let src: &mut dyn EditSource = &mut rsrc;
        #[cfg(feature = "observe")]
        crate::record::api(&self.core.values, "evaluate_begin", serde_json::Value::Null, serde_json::Value::Null);
        let r = self.evaluate_inner(sched, exec, src);
        #[cfg(feature = "observe")]
        {
            let r = match &r {
                Ok(rep) => serde_json::json!({"ok": {"nodes_run": rep.nodes_run, "rounds": rep.rounds}}),
                Err(e) => serde_json::json!(e.to_string()),
            };
            crate::record::api(&self.core.values, "evaluate_end", serde_json::Value::Null, r);
        }
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
        // (No run is out: no late filing for a node gone.)
        self.w.rd.purge_gone();
        (self.t_op, self.t_commit, self.t_idle) = (0, 0, 0);
        self.t_seen = (0, 0, 0);
        self.stopped = 0;
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
        self.core.hold_fin.store(self.loop_start != LoopStart::Previous, std::sync::atomic::Ordering::Relaxed);
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
            epoch: super::core::now(),
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
                    // (All at once: one pass over the runs' reads.)
                    self.declare_loopbacks(&later);
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
                        // Another round: final nodes wait for the final
                        // phase from here (the dirty ones as well).
                        if !self.core.hold_fin.swap(true, std::sync::atomic::Ordering::Relaxed) {
                            self.hold_dirty_finals();
                        }
                        if per_round {
                            if let Some((w, d)) = self.ev_olds.pop_front() {
                                let done = std::mem::replace(&mut self.w, w);
                                self.ev_prev = Some(done.clone());
                                self.ev_done.push((done, Vec::new()));
                                s = s.fresh();
                                self.start_world(&mut s, &d);
                            } else {
                                self.ev_done.push((self.w.snapshot(), Vec::new()));
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
                let held = !self.w.fins_in(St::Held).is_empty();
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
                        self.ev_done.push((self.w.snapshot(), Vec::new()));
                    }
                }
                // Held nodes run; final nodes the final phase itself places
                // (held when made) run in turn, until none is left.
                rep.final_nodes_run += 1;
                let mut once = false;
                loop {
                    let held: Vec<Id> = self.w.fins_in(St::Held);
                    // (Drained once at least: the final world's queued work.)
                    if held.is_empty() && once {
                        break;
                    }
                    once = true;
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
            // (The final phase's time goes with the last round.)
            let (o, c, i) = self.t_seen;
            t.op_ns += self.t_op - o;
            t.commit_ns += self.t_commit - c;
            t.idle_ns += self.t_idle - i;
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
        let chain = if hs.on { chain_of(&hs) } else { Vec::new() };
        rep.critical_links = link_times(&chain);
        self.chain_at = chain.iter().map(|c| (c.0, c.3)).collect();
        self.chain = chain.into_iter().map(|c| (c.0, c.1, c.2)).collect();
        rep.critical_path = self.chain.first().map_or(0, |c| c.1);
        rep.realised_critical_path = self.chain.first().map_or(0, |c| c.1 + 1);
        rep.realised_critical_time = std::time::Duration::from_nanos(hs.t.values().copied().max().unwrap_or(0));
        let mut causes: Vec<(NodeId, crate::api::Rerun)> = std::mem::take(&mut *super::core::lock(&self.core.causes))
            .into_iter()
            .map(|(id, c)| (NodeId(id), c))
            .collect();
        causes.sort_by_key(|c| c.0);
        rep.rerun_causes = causes;
        // (Complete runs: a stopped attempt is not one; the limits count
        // attempts.)
        rep.runs -= self.stopped;
        self.core.hold_fin.store(self.loop_start != LoopStart::Previous, std::sync::atomic::Ordering::Relaxed);
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
        if let Some(t) = super::core::now() {
            s.ready_at.insert(id, t);
        }
        if let Some(v) = s.lane_new.as_mut() {
            v.push(id);
        }
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
        // steps ~350-430 ms. CLEAN_FLOW=2: the combining merge.)
        static FLOW: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
        let flow = *FLOW.get_or_init(|| match std::env::var("CLEAN_FLOW").as_deref() {
            Ok("2") => 2,
            Ok("0") | Err(_) => 0,
            Ok(_) => 1,
        });
        if exec.parallelism() > 1 && flow == 2 {
            return self.drain_flow2(s, rep, ran, cut, deadline, hs, src, per_round, rounds, sched, exec);
        }
        if exec.parallelism() > 1 && flow == 1 {
            return self.drain_flow(s, rep, ran, cut, deadline, hs, src, per_round, rounds, sched, exec);
        }
        if exec.parallelism() > 1 || exec.steps() {
            return super::core::prof::time(24, || self.drain_steps(s, rep, ran, cut, deadline, hs, src, per_round, rounds, sched, exec));
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
            self.t_op += u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
            let applied = matches!(outcome, Outcome::Done(_));
            let tc = std::time::Instant::now();
            super::core::prof::time(1, || self.apply(s, rep, ran, cut, hs, id, outcome, true))?;
            self.t_commit += u64::try_from(tc.elapsed().as_nanos()).unwrap_or(u64::MAX);
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
        // (CLEAN_WINDOW: the ready window's size, to study.)
        static WINDOW: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
        let window = WINDOW
            .get_or_init(|| std::env::var("CLEAN_WINDOW").ok().and_then(|w| w.parse().ok()))
            .unwrap_or_else(|| (exec.parallelism().max(1) * 8).max(64));
        let mut idle: u32 = 0;
        let predict = sched.speculation_budget() > 0;
        loop {
            if s.queue.is_empty() {
                if self.requeue_hint_parked(s) {
                    continue;
                }
                break;
            }
            rep.max_ready = rep.max_ready.max(s.queue.len());
            self.check_deadline(s, rep, deadline)?;
            let Some((mut chosen, mut resumes, fueled)) = super::core::prof::time(22, || self.select_batch(s, rep, ran, cut, hs, window, predict, sched, &FxHashSet::default()))? else { continue };
            // Run against the snapshot; the results merge in text order
            // after the batch (merging in the workers as results came only
            // added contention: 2026-10-20, para4000 j8 341 -> 430 ms).
            chosen.sort_by_key(|&id| self.w.node(id).map(|n| (n.unit, n.pos)));
            let snap = self.w.clone();
            let jobs: Vec<(Id, std::sync::Mutex<Kept>)> = chosen
                .iter()
                .map(|&id| (id, std::sync::Mutex::new(resumes.remove(&id).flatten())))
                .collect();
            let fuel = sched.fuel();
            // A node refuted this evaluation runs on committed values only:
            // no predictions, and a definer in work is waited for.
            let careful: FxHashSet<Id> = chosen.iter().copied().filter(|x| s.refutes.contains_key(x)).collect();
            let slots: Vec<std::sync::Mutex<Option<(Outcome, std::time::Duration)>>> = (0..jobs.len()).map(|_| std::sync::Mutex::new(None)).collect();
            let core = std::sync::Arc::clone(&self.core);
            // The lane (CLEAN_LANE): the first job runs on the world itself
            // and commits at once, then runs what its commits queued while
            // the step's other jobs run (a chain goes on without waiting
            // for the step's end).
            let par = exec.parallelism().max(1);
            let lane_on = lane() && par > 1;
            let mut m = Merge {
                eng: &mut *self,
                s: &mut *s,
                rep: &mut *rep,
                ran: &mut *ran,
                cut: &mut *cut,
                hs: &mut *hs,
                next: usize::from(lane_on),
                applied: 0,
                parked: 0,
                err: None,
            };
            {
                let left = std::sync::atomic::AtomicUsize::new(jobs.len() - 1);
                let busy_ids: FxHashSet<Id> = jobs.iter().skip(1).map(|j| j.0).collect();
                let lane_busy = std::sync::atomic::AtomicU64::new(0);
                let lane_ran = std::sync::atomic::AtomicBool::new(false);
                let lane_m = std::sync::Mutex::new(&mut m);
                let run = |i: usize| {
                    if lane_on && i == 0 {
                        lane_ran.store(true, std::sync::atomic::Ordering::Relaxed);
                        let mut g = super::core::lock(&lane_m);
                        let d = g.lane(&core, &snap, &jobs, &slots, &careful, &fueled, fuel, predict, &busy_ids, &left, par, deadline);
                        lane_busy.fetch_add(d, std::sync::atomic::Ordering::Relaxed);
                        return crate::api::RunResult(());
                    }
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
                    left.fetch_sub(1, std::sync::atomic::Ordering::Release);
                    crate::api::RunResult(())
                };
                let mut out = Vec::with_capacity(jobs.len());
                let ts = std::time::Instant::now();
                super::core::prof::time(23, || exec.execute(jobs.len(), &run, &mut out));
                // (Idle: the workers' time in the step not spent in runs.)
                let busy: u64 = slots.iter().filter_map(|x| super::core::lock(x).as_ref().map(|(_, d)| u64::try_from(d.as_nanos()).unwrap_or(0))).sum::<u64>() + lane_busy.into_inner();
                let span = u64::try_from(ts.elapsed().as_nanos()).unwrap_or(u64::MAX).saturating_mul(exec.parallelism().max(1) as u64);
                m.eng.t_op += busy;
                m.eng.t_idle += span.saturating_sub(busy);
                // (The executor dropped the lane: its node runs again.)
                if lane_on && !lane_ran.into_inner() {
                    let (id, resume) = &jobs[0];
                    if let Some(r) = super::core::lock(resume).take() {
                        m.eng.kept.insert(*id, r);
                    }
                    m.eng.enqueue(m.s, *id);
                }
            }
            // The field writes of the commits that change no structure are
            // recorded and made together, in parallel ([`super::batch`]);
            // a step of a few results commits directly (recording and a
            // flush cost more than they save there).
            m.eng.defer = (jobs.len() >= 4).then(super::batch::Pending::default);
            let tc = std::time::Instant::now();
            if m.err.is_none() {
                m.drain(&snap, &jobs, &slots, exec);
            }
            m.flush(exec);
            m.eng.defer = None;
            m.eng.t_commit += u64::try_from(tc.elapsed().as_nanos()).unwrap_or(u64::MAX);
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

    /// The final nodes waiting to run in the round: held for the final
    /// phase instead.
    fn hold_dirty_finals(&mut self) {
        let ids: Vec<Id> = self.w.fins_in(St::Dirty);
        for id in ids {
            if let Some(mut n) = self.w.node(id).cloned() {
                n.st = St::Held;
                self.w.put(id, n);
                self.w.ver = super::live::fresh_epoch();
            }
        }
    }

    /// Nodes parked by their hints whose blocker left work without waking
    /// them, queued again (when the queue ran dry); whether there were any.
    fn requeue_hint_parked(&self, s: &mut Sched) -> bool {
        let back: Vec<Id> = std::mem::take(&mut s.hint_parked)
            .into_iter()
            .filter(|(id, x)| s.parked.get(id) == Some(&Wait::Node(*x, None)))
            .map(|(id, _)| id)
            .collect();
        for &id in &back {
            s.parked.remove(&id);
            self.enqueue(s, id);
        }
        !back.is_empty()
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
        loop {
            if s.queue.is_empty() && !self.requeue_hint_parked(s) {
                break;
            }
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
                        let ahead = g.m.eng.w.node(id).is_some_and(|n| g.m.eng.w.work_of(n.unit).is_some_and(|m| m.below(&n.pos).is_some()));
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

    /// Runs dirty nodes on the executor's workers without steps, merging
    /// by combining (CLEAN_FLOW=2): a worker that finishes hands its result
    /// in and takes the merge if it is free (never waiting for it); the
    /// holder merges every result handed in that no running job comes
    /// before in text order, picks the next ready nodes, publishes the
    /// world once (the snapshot jobs then start on) and lets go, taking it
    /// again if a result came in meanwhile. Workers wait only for work.
    /// Edits are taken in between epochs of a few hundred merges.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn drain_flow2(
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
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let par = exec.parallelism().max(1);
        let window = (par * 8).max(64);
        let predict = sched.speculation_budget() > 0;
        let fuel = sched.fuel();
        let mut idle: u32 = 0;
        loop {
            if s.queue.is_empty() && !self.requeue_hint_parked(s) {
                break;
            }
            rep.max_ready = rep.max_ready.max(s.queue.len());
            self.check_deadline(s, rep, deadline)?;
            let core = std::sync::Arc::clone(&self.core);
            let q = std::sync::Mutex::new(FlowQ {
                ready: std::collections::BTreeMap::new(),
                running: FxHashSet::default(),
                done: Vec::new(),
                latest: std::sync::Arc::new(self.w.clone()),
                stop: false,
            });
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
            // A result was handed in (or the first pick is due): whoever
            // finds the merge free takes it.
            let want = AtomicBool::new(true);
            let budget = 256usize;
            // (Round times: ops' and the merge's ns, summed over workers.)
            let (op_ns, merge_ns) = (std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0));
            let run = |_: usize| {
                loop {
                    // (The holder looks again after letting go: a result
                    // handed in while it held the merge is not left.)
                    while want.load(SeqCst) {
                        let Ok(mut g) = flow.try_lock() else { break };
                        want.store(false, SeqCst);
                        let t = super::core::now();
                        g.combine(&q, &cv, par, window, predict, budget, deadline);
                        merge_ns.fetch_add(super::core::ns_since(t), SeqCst);
                    }
                    let mut qg = super::core::lock(&q);
                    if qg.stop {
                        break;
                    }
                    if let Some((_, (id, resume, careful, fueled))) = qg.ready.pop_first() {
                        qg.running.insert(id);
                        let snap = std::sync::Arc::clone(&qg.latest);
                        drop(qg);
                        let got = if snap.node(id).is_some_and(|n| n.st == St::Dirty) {
                            #[cfg(not(target_arch = "wasm32"))]
                            let t = std::time::Instant::now();
                            let o = super::core::prof::time(0, || run_job(&core, &snap, id, resume, predict && !careful, careful, fueled.then_some(fuel)));
                            #[cfg(not(target_arch = "wasm32"))]
                            let d = t.elapsed();
                            #[cfg(target_arch = "wasm32")]
                            let d = std::time::Duration::ZERO;
                            op_ns.fetch_add(u64::try_from(d.as_nanos()).unwrap_or(u64::MAX), SeqCst);
                            Ok((o, d))
                        } else {
                            Err(resume)
                        };
                        let mut qg = super::core::lock(&q);
                        qg.running.remove(&id);
                        qg.done.push((id, snap, got));
                        drop(qg);
                        want.store(true, SeqCst);
                        continue;
                    }
                    // (A result handed in meanwhile is the holder's: it looks
                    // again after letting go; waiting here, not spinning.)
                    if want.load(SeqCst) && flow.try_lock().is_ok() {
                        continue;
                    }
                    drop(cv.wait(qg).unwrap_or_else(std::sync::PoisonError::into_inner));
                }
                crate::api::RunResult(())
            };
            let mut out = Vec::with_capacity(par);
            exec.execute(par, &run, &mut out);
            let mut f = flow.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
            f.m.eng.t_op += op_ns.into_inner();
            f.m.eng.t_commit += merge_ns.into_inner();
            let qd = q.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
            // (Jobs no worker took, or chosen and not handed out: not
            // runs; their nodes run again.)
            let left = qd.ready.into_values().map(|(id, r, _, _)| (id, r));
            for (id, resume) in f.buf.drain(..).chain(left) {
                f.m.eng.unrun(f.m.rep, id);
                if let Some(r) = resume {
                    f.m.eng.kept.insert(id, r);
                }
                f.m.eng.enqueue(f.m.s, id);
            }
            for (id, ..) in qd.done {
                f.m.eng.enqueue(f.m.s, id);
            }
            let (applied, parked, err) = (f.m.applied, f.m.parked, f.m.err.take());
            drop(f);
            if let Some(e) = err {
                return Err(e);
            }
            rep.steps += 1;
            width(rep, applied);
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
    ) -> Result<Option<(Vec<Id>, FxHashMap<Id, Kept>, FxHashSet<Id>)>, EvalError> {
        // The ready nodes: the first of the queue in text order whose
        // read hints do not wait on a node in work (learned waits); at
        // least the first.
        let mut ready: Vec<(u32, Pos, Id)> = Vec::new();
        let mut resumes: FxHashMap<Id, Kept> = FxHashMap::default();
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
            if !first && let Err(x) = self.w.hint_blocker(&self.core, k.2, predict) {
                // (Parked on a dirty blocker: its commit queues it again;
                // a scan per step (or pick) for each cost a window's worth
                // of checks, growing with the threads.)
                if let Some(x) = x
                    && x != k.2
                    && self.w.node(x).is_some_and(|n| n.st == St::Dirty)
                {
                    if super::core::debug() {
                        let op = |y: Id| self.w.node(y).map(|n| self.core.op_name(n.text.op));
                        eprintln!("held {} ({:?}) on {x} ({:?})", k.2, op(k.2), op(x));
                    }
                    s.queue.remove(&k);
                    s.queued.remove(&k.2);
                    s.parked.insert(k.2, Wait::Node(x, None));
                    s.waiters.entry(x).or_default().push(k.2);
                    s.hint_parked.insert(k.2, x);
                }
                continue;
            }
            // A node refuted this evaluation runs in text order: when
            // nothing before it in its unit is in work (or it is first).
            if !first && s.refutes.contains_key(&k.2) && self.w.work_of(k.0).is_some_and(|m| m.below(&k.1).is_some()) {
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
            fold_redo: false,
            span: None,
            base: None,
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
    /// Placer `p` starts over, its kept state broken from position `from`
    /// on: what it placed past there waits for it, as for a placer a change
    /// dirtied. (Up to there its next attempt reads what this one read and
    /// places the same: those go on, or a page builder restarted by a
    /// change past its finishers would take them away and back.)
    fn stale_placer(&mut self, p: Id, from: Pos) {
        if let Some(r) = self.w.node(p).and_then(|n| n.res.clone()) {
            for &x in r.kids.iter().chain(&r.conts) {
                if self.w.node(x).is_none_or(|xn| xn.pos > from) {
                    self.w.awaiting.insert(x, (), &NoMeasure);
                }
            }
        }
    }

    /// Where placer `p`'s broken read `r` starts to differ: a range read's
    /// window start (what it read before it stands), else anywhere.
    fn broken_from(&self, p: Id, r: &Read) -> Pos {
        let pu = self.w.node(p).map(|n| n.unit);
        match r {
            Read::Range { from, .. } => self.w.node(*from).filter(|n| Some(n.unit) == pu).map_or(0, |n| n.pos),
            _ => 0,
        }
    }

    /// Where placer `p`'s kept state stands on a read that no longer holds
    /// (the first one's start, see [`Self::broken_from`]); asked once per
    /// world version while they hold.
    fn kept_broken(&self, s: &mut Sched, p: Id) -> Option<Pos> {
        let (_, reads, _) = self.kept.get(&p)?;
        if s.kept_ok.get(&p) == Some(&self.w.ver) {
            return None;
        }
        match reads.iter().find(|r| !super::build::holds(&self.core, &self.w, p, r)) {
            Some(r) => Some(self.broken_from(p, r)),
            None => {
                s.kept_ok.insert(p, self.w.ver);
                None
            }
        }
    }

    fn prepare(&mut self, s: &mut Sched, rep: &mut Report, k: (u32, Pos, Id)) -> Option<Kept> {
        let id = k.2;
        s.queued.remove(&id);
        if let Some(&t) = s.ready_at.get(&id) {
            s.ran_ready.insert(id, t);
        }
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
        if let Some((f, from, to, past)) = s.range_wait.remove(&id)
            && let Some((u, lo, hi)) = self.w.window(id, from, to, past)
            && let Some(x) = self.w.range_blocker_last(id, f, u, lo, hi)
        {
            s.range_wait.insert(id, (f, from, to, past));
            s.parked.insert(id, Wait::Node(x, Some(f)));
            s.waiters.entry(x).or_default().push(id);
            return None;
        }
        // Placed by a placer that runs again for a change: it waits for the
        // placer to place it again (as it was: it runs; with new arguments:
        // a new text; a run that ends without it: it goes).
        let placer = n.parent;
        // A placer stopped on a kept state whose reads broke (a node it
        // read went) is such a placer too: it starts over now (its kept
        // state is of a world before), and what it placed waits for it.
        if self.kept.contains_key(&placer)
            && let Some(from) = self.kept_broken(s, placer)
        {
            self.kept.remove(&placer);
            s.kept_ok.remove(&placer);
            s.parked.remove(&placer);
            s.range_wait.remove(&placer);
            self.enqueue(s, placer);
            self.stale_placer(placer, from);
        }
        if self.w.awaiting.contains_key(&id) && self.w.node(placer).is_some_and(|p| super::world::in_work(p.st)) {
            s.parked.insert(id, Wait::Node(placer, None));
            s.placed.entry(placer).or_default().push(id);
            rep.stops += 1;
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
            Some((tok, reads, base)) => match reads.iter().find(|r| !super::build::holds(&self.core, &self.w, id, r)) {
                None => {
                    rep.resumed += 1;
                    Some((tok, reads, base))
                }
                Some(r) => {
                    rep.restarted += 1;
                    bump(&mut rep.restarted_by, read_kind(r));
                    // (Starting over, it is a stale placer.)
                    let from = self.broken_from(id, r);
                    self.stale_placer(id, from);
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
        let t = super::core::now();
        let r = self.apply_one(s, rep, ran, cut, hs, id, outcome, fresh);
        rep.merge.total += super::core::ns_since(t);
        rep.merged += 1;
        r
    }

    /// [`Self::apply`] untimed.
    #[allow(clippy::too_many_arguments)]
    fn apply_one(
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
                            Some(crate::api::Rerun::Range { field: Some(f) }) if fold_guess(&self.core, s, id, f, true) => {
                                rep.mispredictions += 1;
                                bump_field(&mut rep.refuted_by_field, f);
                            }
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
                        let t_ns = s.time.get(&id).map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
                        let t = super::core::now();
                        let span = r.span.map(|(a, b)| (s.ran_ready.get(&id).copied().unwrap_or(a).min(a), a, b));
                        self.height(hs, id, &r.res, r.fold_redo, span, t_ns);
                        rep.merge.heights += super::core::ns_since(t);
                    }
                    let dbg = super::core::debug().then(|| format!("{:?} out {}", r.res.reads, self.core.values.render(r.res.out)));
                    let w = std::mem::take(&mut self.w);
                    let mut b = Builder::new(&self.core, w);
                    b.app_env(id, &r.env);
                    b.defer = self.defer.take();
                    b.cont = r.base.map(|x| (x.0, x.1));
                    let changed = super::core::prof::time(9, || b.commit(id, r.res, &r.kids, &r.conts));
                    let mut m = b.finish();
                    self.defer = m.defer.take();
                    if let Some(d) = dbg {
                        eprintln!("run {id} reads {d} dirty {:?} woken {:?}", m.dirty, m.woken);
                    }
                    m.counts.add_to(rep);
                    rep.merge.add(&m.counts.merge);
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
                    if hs.on {
                        hs.committed(id);
                    }
                    let t = super::core::now();
                    for x in &freed {
                        self.kept.remove(x);
                        s.parked.remove(x);
                        s.env.remove(x);
                        // (Its waiters: their subject is gone.)
                        self.wake(s, *x);
                    }
                    for x in dirty {
                        self.enqueue(s, x);
                    }
                    for x in woken {
                        self.wake(s, x);
                    }
                    rep.merge.wake += super::core::ns_since(t);
                    if let Some(e) = self.hook_error() {
                        return Err(e);
                    }
                }
                Outcome::Stopped(r, wait) => {
                    rep.stops += 1;
                    self.stopped += 1;
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
                        self.w.ver = super::live::fresh_epoch();
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
                        self.w.ver = super::live::fresh_epoch();
                        self.w.refresh_holder(id);
                        self.w.refresh_claim_at(id);
                        self.wake(s, id);
                    }
                    if super::core::debug() {
                        eprintln!("stop {id} on {wait:?} reads {:?} node {:?}", r.res.reads, self.w.node(match wait { Wait::Node(x, _) => x, _ => id }));
                    }
                    if let Some(k) = r.kept {
                        // (The placements made so far, this attempt's
                        // committed below.)
                        let nk = r.kids.iter().filter(|x| !matches!(x.place, super::world::Place::Marker(_))).count();
                        let b = r.base.unwrap_or_default();
                        self.kept.insert(id, (k, r.res.reads.clone(), (b.0 + r.kids.len(), b.1 + r.conts.len(), b.2 + nk)));
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
                    // Its placements so far are made now, whatever it stops
                    // on (a child it waits for, or a page builder parked
                    // between attempts whose finishers can run meanwhile):
                    // its next run places them again, the same nodes.
                    // What it still holds before the node it now waits on and
                    // did not place again goes: this attempt is past it (it
                    // would run on arguments from before, and the wait may
                    // stand on it: Pages waits on a node that waits on a
                    // finisher it holds).
                    let unfold = matches!(wait, Wait::Child(_)) || !r.kids.is_empty() || !r.conts.is_empty();
                    let past = |w: &World| match &wait {
                        Wait::Node(x, _) if !w.awaiting.is_empty() => super::build::passed(w, id, *x),
                        _ => Vec::new(),
                    };
                    if unfold || !past(&self.w).is_empty() {
                        if unfold && let Some(e) = self.runaway(id, &r.kids, &r.conts) {
                            return Err(e);
                        }
                        let w = std::mem::take(&mut self.w);
                        let mut b = Builder::new(&self.core, w);
                        if unfold {
                            b.cont = r.base.map(|x| (x.0, x.1));
                            b.commit_unfolding(id, &r.kids, &r.conts);
                            b.cont = None;
                        }
                        let gone = past(&b.w);
                        if !gone.is_empty() {
                            b.free_placed(id, &gone);
                        }
                        let m = b.finish();
                        let (dirty, created, freed, woken) = (m.dirty, m.created, m.freed, m.woken);
                        self.rebinds += m.rebinds;
                        self.w = m.w;
                        self.created += created;
                        self.freed += freed.len() as u64;
                        for x in &freed {
                            self.kept.remove(x);
                            s.parked.remove(x);
                            s.env.remove(x);
                            self.wake(s, *x);
                        }
                        for x in dirty {
                            self.enqueue(s, x);
                        }
                        for x in woken {
                            self.wake(s, x);
                        }
                    }
                    // (Stopped, its placements so far made again: those go
                    // on; a node it placed and now waits for itself goes on
                    // too, as it stands, or neither would.)
                    if let Wait::Node(x, _) = &wait
                        && self.w.awaiting.contains_key(x)
                        && self.w.node(*x).is_some_and(|xn| xn.parent == id)
                    {
                        self.w.awaiting.remove(x, &NoMeasure);
                    }
                    self.wake_placed(s, id);
                    let subject = match &wait {
                        Wait::Child(k) => self
                            .w
                            .node(id)
                            .and_then(|n| n.res.as_ref())
                            .and_then(|r| r.kids.get(*k).copied()),
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
    /// (A range read is over the window's last contributor as it last
    /// contributed; none on a `fold_redo` commit, whose folds waited for
    /// nothing.)
    fn height(&self, hs: &mut Heights, id: Id, res: &Res, fold_redo: bool, span: Option<(std::time::Instant, std::time::Instant, std::time::Instant)>, t_ns: u64) {
        use crate::api::Cause;
        let mut best: Option<(u32, Id, Cause)> = None;
        let mut best_t = 0u64;
        let mut offer = |hs: &Heights, x: Id, c: Cause| {
            // (A range's contributor: as it last contributed.)
            let at = match c {
                Cause::Range { .. } => hs.hc.get(&x).map(|e| (e.0.0, e.1)),
                _ => None,
            };
            if let Some((h, t)) = at.or_else(|| hs.h.get(&x).map(|e| (e.0, hs.t.get(&x).copied().unwrap_or(0)))) {
                best_t = best_t.max(t);
                if best.as_ref().is_none_or(|b| h > b.0) {
                    best = Some((h, x, c));
                }
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
                // (A range's last contributor: a chain through an
                // accumulator falls by one through each.)
                Read::Range { f, from, to, past, .. } => {
                    if !fold_redo
                        && let Some((u, lo, hi)) = self.w.window(id, *from, *to, *past)
                        && let Some((p, _)) = self.w.field(u, *f).map(|c| &c.contribs).and_then(|m| m.below(&hi)).filter(|(p, _)| **p > lo)
                        && let Some(&x) = self.w.unit(u).order.get(p)
                        && x != id
                    {
                        offer(hs, x, Cause::Range {
                            field: *f,
                            from: Some(NodeId(x)),
                        });
                    }
                }
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
        let rel = |t: std::time::Instant| hs.epoch.map_or(0, |e| u64::try_from(t.saturating_duration_since(e).as_nanos()).unwrap_or(u64::MAX));
        let at = span.map_or([0; 4], |(q, a, b)| [rel(q), rel(a), rel(b), rel(b)]);
        hs.at.insert(id, at);
        let same = self.w.node(id).and_then(|n| n.res.as_ref()).is_some_and(|old| old.contribs == res.contribs);
        hs.hc_new = None;
        if !same || !hs.hc.contains_key(&id) {
            hs.hc.insert(id, (v.clone(), best_t + t_ns, at));
            hs.hc_new = Some(id);
        }
        hs.h.insert(id, v);
        hs.t.insert(id, best_t + t_ns);
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
            if self.w.work_of(n.unit).is_some_and(|m| m.below(&n.pos).is_some()) {
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
        self.wake_placed(s, x);
    }

    /// The nodes waiting for placer `x`'s placements: they go on.
    fn wake_placed(&self, s: &mut Sched, x: Id) {
        if let Some(v) = s.placed.remove(&x) {
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
    fn round_done(&mut self, rep: &mut Report, round_runs: &mut u64, from_old: bool) {
        let runs = rep.runs - *round_runs;
        *round_runs = rep.runs;
        let memo_hits = if from_old { (self.w.nodes.len() as u64).saturating_sub(runs) } else { 0 };
        let (o, c, i) = self.t_seen;
        rep.round_times.push(crate::api::RoundTimes {
            runs,
            memo_hits,
            op_ns: self.t_op - o,
            commit_ns: self.t_commit - c,
            idle_ns: self.t_idle - i,
            ..Default::default()
        });
        self.t_seen = (self.t_op, self.t_commit, self.t_idle);
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
        let left: Vec<Id> = self.w.dirty_nodes();
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

}

/// Adds one to `name`'s count in `list`.
fn bump(list: &mut Vec<(&'static str, u64)>, name: &'static str) {
    match list.iter_mut().find(|x| x.0 == name) {
        Some(x) => x.1 += 1,
        None => list.push((name, 1)),
    }
}

/// The chain's time by link: each run against the one below it (`chain`
/// is highest first).
fn link_times(chain: &[(NodeId, u32, crate::api::Cause, [u64; 4])]) -> crate::api::LinkTimes {
    let mut t = crate::api::LinkTimes::default();
    for (k, c) in chain.iter().enumerate() {
        let [ready, start, end, commit] = c.3;
        let below = chain.get(k + 1).map_or(0, |b| b.3[3]);
        // (Queued before the one below committed: ready since.)
        let ready = ready.max(below);
        t.held += ready.saturating_sub(below);
        t.dispatch += start.saturating_sub(ready);
        t.op += end.saturating_sub(start);
        t.wait_merge += commit.saturating_sub(end);
        t.links += 1;
    }
    t
}

/// The critical chain: from the highest run down its links, with each
/// run's queueing, start, end and commit.
fn chain_of(hs: &Heights) -> Vec<(NodeId, u32, crate::api::Cause, [u64; 4])> {
    let Some((&top, _)) = hs.h.iter().max_by_key(|(id, v)| (v.0, std::cmp::Reverse(**id))) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut x = Some(top);
    // (Through a range link, the contributor as it last contributed.)
    let mut via_range = false;
    while let Some(id) = x {
        let e = if via_range { hs.hc.get(&id).map(|e| (&e.0, e.2)) } else { None };
        let Some(((h, next, c), at)) = e.or_else(|| hs.h.get(&id).map(|e| (e, hs.at.get(&id).copied().unwrap_or_default()))) else { break };
        out.push((NodeId(id), *h, c.clone(), at));
        via_range = matches!(c, crate::api::Cause::Range { .. });
        x = *next;
        if out.len() > hs.h.len() + hs.hc.len() {
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
    for (id, _) in w.publishers.iter() {
        let Some(r) = w.node(*id).and_then(|n| n.res.as_ref()) else { continue };
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
    buf: std::collections::VecDeque<(Id, Kept)>,
    fueled: FxHashSet<Id>,
    inflight: FxHashSet<Id>,
    /// Finished runs: the world each ran on, and what it gave.
    done: Vec<(Id, World, Outcome, std::time::Duration)>,
    merged: usize,
    stop: bool,
}

/// A job ready to start: its node, kept state, whether careful, whether
/// fueled.
type FlowJob = (Id, Kept, bool, bool);

/// A job's run and its time; `Err`: not run (its node no longer dirty on
/// the world it was to run on), its kept state back.
type FlowRun = Result<(Outcome, std::time::Duration), Kept>;

/// CLEAN_FLOW=2's hand-over between workers and the merge (each held
/// briefly): jobs ready to start (by text position), the ones running,
/// results handed in (with the world each ran on), and the world last
/// published.
struct FlowQ {
    ready: std::collections::BTreeMap<(u32, Pos, Id), FlowJob>,
    running: FxHashSet<Id>,
    done: Vec<(Id, std::sync::Arc<World>, FlowRun)>,
    latest: std::sync::Arc<World>,
    stop: bool,
}

impl Flow<'_> {
    /// The next node to run: from the batch chosen last, else a new one.
    fn pick(&mut self, window: usize, predict: bool) -> Option<(Id, Kept)> {
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

    /// CLEAN_FLOW=2's merge, by the worker holding it: merges the results
    /// handed in that no running job comes before in text order (as a
    /// step's merge would), then fills the ready jobs up to the workers,
    /// publishing the world once; at the epoch's end (nothing ready,
    /// running or handed in) everyone stops.
    #[allow(clippy::too_many_arguments)]
    fn combine(&mut self, q: &std::sync::Mutex<FlowQ>, cv: &std::sync::Condvar, par: usize, window: usize, predict: bool, budget: usize, deadline: Option<std::time::Instant>) {
        let key = |w: &World, x: Id| w.node(x).map_or((u32::MAX, Pos::MAX), |n| (n.unit, n.pos));
        let mut merged_any = false;
        loop {
            let mut now = Vec::new();
            {
                let mut qg = super::core::lock(q);
                let w = &self.m.eng.w;
                let first_running = qg.running.iter().map(|&x| key(w, x)).min();
                let mut done = std::mem::take(&mut qg.done);
                done.sort_by_key(|r| std::cmp::Reverse(key(w, r.0)));
                while done.last().is_some_and(|r| first_running.is_none_or(|fr| key(w, r.0) < fr)) {
                    now.push(done.pop().expect("checked"));
                }
                qg.done = done;
            }
            if now.is_empty() {
                break;
            }
            merged_any = true;
            for (id, snap, got) in now {
                self.inflight.remove(&id);
                if self.m.err.is_some() {
                    continue;
                }
                let (o, d) = match got {
                    Ok(r) => r,
                    Err(resume) => {
                        // (Not run: its node went or finished meanwhile;
                        // dirty again, it is queued again.)
                        self.m.eng.unrun(self.m.rep, id);
                        if self.m.eng.w.node(id).is_some_and(|n| n.st == St::Dirty) {
                            if let Some(r) = resume {
                                self.m.eng.kept.insert(id, r);
                            }
                            self.m.eng.enqueue(self.m.s, id);
                        }
                        continue;
                    }
                };
                *self.m.s.time.entry(id).or_default() += d;
                let ahead = self.m.eng.w.node(id).is_some_and(|n| self.m.eng.w.work_of(n.unit).is_some_and(|m| m.below(&n.pos).is_some()));
                if let Err(e) = self.m.one(&snap, usize::from(ahead), id, o, &mut Sequential) {
                    self.m.err = Some(e);
                }
                self.merged += 1;
            }
        }
        if self.m.err.is_none()
            && let Err(e) = self.m.eng.check_deadline(self.m.s, self.m.rep, deadline)
        {
            self.m.err = Some(e);
        }
        self.stop |= self.m.err.is_some();
        // The next jobs, chosen without the hand-over held.
        let (ready, running) = {
            let qg = super::core::lock(q);
            (qg.ready.len(), qg.running.len())
        };
        let mut jobs = Vec::new();
        if !self.stop && self.merged < budget {
            while ready + running + jobs.len() < par + 1 {
                let Some((id, resume)) = self.pick(window, predict) else { break };
                let careful = self.m.s.refutes.contains_key(&id);
                let fueled = self.fueled.remove(&id);
                self.inflight.insert(id);
                jobs.push((key(&self.m.eng.w, id), (id, resume, careful, fueled)));
            }
        }
        let mut qg = super::core::lock(q);
        if merged_any || !jobs.is_empty() {
            qg.latest = std::sync::Arc::new(self.m.eng.w.clone());
        }
        for (k, j) in jobs {
            qg.ready.insert((k.0, k.1, j.0), j);
        }
        if self.stop || (qg.ready.is_empty() && qg.running.is_empty() && qg.done.is_empty()) {
            qg.stop = true;
        }
        drop(qg);
        cv.notify_all();
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

/// A stopped run's kept state, and the reads it made before stopping.
type Kept = Option<(Box<dyn std::any::Any + Send>, Vec<Read>, super::ctx::Base)>;

type Resume = std::sync::Mutex<Kept>;

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

    /// The step's lane: runs `first` on the world as it is and commits it,
    /// then, while the step's other jobs (`busy`) run (`left` of them), or
    /// its commits queued fewer than `par` nodes off the chain, the
    /// chain on from it: the first ready node its commits queued that runs
    /// the first node's operation (a scan's next scan, not the pieces it
    /// unfolded), or the node the last run stopped on; once the other jobs
    /// are done, their results merge first (`snap`, `jobs`, `slots`: the
    /// step's);
    /// the time it ran ops (ns).
    #[allow(clippy::too_many_arguments)]
    fn lane(
        &mut self,
        core: &super::core::Core,
        snap: &World,
        jobs: &[(Id, Resume)],
        slots: &[std::sync::Mutex<Option<(Outcome, std::time::Duration)>>],
        careful: &FxHashSet<Id>,
        fueled: &FxHashSet<Id>,
        fuel: u64,
        predict: bool,
        busy: &FxHashSet<Id>,
        left: &std::sync::atomic::AtomicUsize,
        par: usize,
        deadline: Option<std::time::Instant>,
    ) -> u64 {
        use super::core::prof::count;
        let mut t_ops = 0u64;
        let first = &jobs[0];
        let mut job = Some((first.0, super::core::lock(&first.1).take(), careful.contains(&first.0), fueled.contains(&first.0)));
        self.s.lane_new = Some(Vec::new());
        // (The chain's operation: its first node's. `others`: the nodes its
        // commits queued off the chain, for the next step's workers.)
        let op = self.eng.w.node(first.0).map(|n| n.text.op);
        let mut others: FxHashSet<Id> = FxHashSet::default();
        while let Some((id, resume, careful, fueled)) = job.take() {
            #[cfg(not(target_arch = "wasm32"))]
            let t = std::time::Instant::now();
            let o = super::core::prof::time(0, || run_job(core, &self.eng.w, id, resume, predict && !careful, careful, fueled.then_some(fuel)));
            #[cfg(not(target_arch = "wasm32"))]
            let d = t.elapsed();
            #[cfg(target_arch = "wasm32")]
            let d = std::time::Duration::ZERO;
            t_ops += u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
            *self.s.time.entry(id).or_default() += d;
            let (eng, s, rep) = (&mut *self.eng, &mut *self.s, &mut *self.rep);
            // (What it stopped on, if in work: the chain's next link.)
            let want = match &o {
                Outcome::Stopped(_, Wait::Node(x, _)) if *x != id => Some(*x),
                _ => None,
            };
            let o = match o {
                Outcome::Stopped(r, _) if r.fuel_out => {
                    rep.fuel_outs += 1;
                    s.no_fuel.insert(id);
                    // (Its placements are not made: a continuing token would
                    // stand on them, and goes.)
                    if let Some(k) = r.kept
                        && (!eng.core.cont_placements || (r.kids.is_empty() && r.conts.is_empty()))
                    {
                        eng.kept.insert(id, (k, r.res.reads.clone(), r.base.unwrap_or_default()));
                    }
                    eng.enqueue(s, id);
                    None
                }
                o => Some(o),
            };
            if let Some(o) = o {
                let done = matches!(o, Outcome::Done(_));
                if let Err(e) = super::core::prof::time(1, || eng.apply(s, rep, self.ran, self.cut, self.hs, id, o, true)) {
                    self.err = Some(e);
                    break;
                }
                if done {
                    self.applied += 1;
                } else if !s.queued.contains(&id) {
                    self.parked += 1;
                }
            }
            // (The step's other jobs done and enough queued to feed the
            // workers: the step ends; while too little is, the chain
            // goes on here rather than a step per link.)
            if left.load(std::sync::atomic::Ordering::Acquire) == 0 {
                // (The step's other results merge now, before the chain
                // goes on: waiting for the step's end, they were checked
                // against a world the lane had moved on and refuted.)
                if self.next < jobs.len() {
                    self.drain(snap, jobs, slots, &mut Sequential);
                    if self.err.is_some() {
                        break;
                    }
                }
                if others.len() >= par {
                    count(24);
                    break;
                }
            }
            if let Err(e) = self.eng.check_deadline(self.s, self.rep, deadline) {
                self.err = Some(e);
                break;
            }
            match self.lane_next(busy, predict, op, want, &mut others) {
                Ok(Some((x, r))) => {
                    count(22);
                    job = Some((x, r, self.s.refutes.contains_key(&x), false));
                }
                Ok(None) => {}
                Err(e) => {
                    self.err = Some(e);
                    break;
                }
            }
        }
        self.s.lane_new = None;
        t_ops
    }

    /// The lane's next node: the node `want` the last run stopped on, if
    /// queued, else the first, in text order, of those its commits queued
    /// that runs operation `op`, when ready to run now (prepared), if any.
    #[allow(clippy::type_complexity)]
    fn lane_next(
        &mut self,
        busy: &FxHashSet<Id>,
        predict: bool,
        op: Option<crate::api::OpId>,
        want: Option<Id>,
        others: &mut FxHashSet<Id>,
    ) -> Result<Option<(Id, Kept)>, EvalError> {
        use super::core::prof::count;
        let (eng, s, rep) = (&mut *self.eng, &mut *self.s, &mut *self.rep);
        if let Some(x) = want
            && s.queued.contains(&x)
            && !busy.contains(&x)
            && let Some(v) = s.lane_new.as_mut()
        {
            v.push(x);
        }
        loop {
            let mut cands = s.lane_new.take().unwrap_or_default();
            cands.retain(|x| s.queued.contains(x) && !busy.contains(x));
            let chain = |x: Id| Some(x) == want || eng.w.node(x).is_some_and(|n| Some(n.text.op) == op);
            others.extend(cands.iter().copied().filter(|&x| !chain(x)));
            let best = cands.iter().filter(|&&x| chain(x)).filter_map(|&x| eng.w.node(x).map(|n| (n.unit, n.pos, x))).filter(|k| s.queue.contains(k)).min();
            let Some(k) = best else {
                s.lane_new = Some(cands);
                count(23);
                return Ok(None);
            };
            cands.retain(|&x| x != k.2);
            s.lane_new = Some(cands);
            // (Held by its hints, or refuted and behind a node in work: it
            // waits for the step.)
            let behind = s.refutes.contains_key(&k.2) && eng.w.work_of(k.0).is_some_and(|m| m.below(&k.1).is_some());
            if behind || eng.w.hint_blocker(&eng.core, k.2, predict).is_err() {
                count(25);
                return Ok(None);
            }
            s.queue.remove(&k);
            let Some(r) = eng.prepare(s, rep, k) else { continue };
            if r.is_none()
                && let Some(run) = eng.reuse(k.2)
            {
                eng.unrun(rep, k.2);
                eng.apply(s, rep, self.ran, self.cut, self.hs, k.2, Outcome::Done(run), true)?;
                continue;
            }
            return Ok(Some((k.2, r)));
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
            rep.merged += 1;
            return Ok(());
        }
        // Re-texted by an earlier commit of this step (a placer's): the run
        // is of the old text.
        let same_text = match (snap.node(id), eng.w.node(id)) {
            (Some(a), Some(b)) => std::sync::Arc::ptr_eq(&a.text, &b.text),
            _ => false,
        };
        if !same_text {
            rep.merged += 1;
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
        // A result whose only broken reads are guessed folds
        // (`Monoid::speculate`), placing nothing: committed for its
        // contributions (which do not depend on what it read), and run
        // again; refuted, they would stay unseen and its step's later
        // readers guess wrong again, one commit a step.
        let tv = super::core::now();
        // (The world as the run saw it: its reads hold, but the
        // environment's, which is not the world's.)
        let same = snap.same_as(&eng.w);
        rep.merge.unchanged += u64::from(same);
        let asked = |rd: &Read| !same || matches!(rd, Read::Env { .. });
        let redo = match &o {
            Outcome::Done(r) if r.kids.is_empty() && r.conts.is_empty() => {
                let speculated = |rd: &Read| matches!(rd, Read::Range { f, .. } if eng.core.acc(*f).is_some_and(|m| m.speculate()));
                let mut broken = r.res.reads.iter().filter(|rd| asked(rd) && !super::build::holds(&eng.core, &eng.w, id, rd));
                match broken.next() {
                    Some(rd) if speculated(rd) && broken.all(speculated) => {
                        let Read::Range { f, .. } = rd else { unreachable!("a fold") };
                        fold_guess(&eng.core, s, id, *f, false).then(|| (*f, super::build::why_broken(&eng.core, &eng.w, id, rd)))
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        let o = match o {
            // (Counted when it commits again.)
            Outcome::Done(mut r) if redo.is_some() => {
                let v = super::core::ns_since(tv);
                (rep.merge.validate, rep.merge.total) = (rep.merge.validate + v, rep.merge.total + v);
                r.fold_redo = true;
                Outcome::Done(r)
            }
            Outcome::Stopped(r, _) if r.fuel_out => {
                rep.merged += 1;
                rep.fuel_outs += 1;
                s.no_fuel.insert(id);
                // (Its placements are not made: a continuing token would
                // stand on them, and goes.)
                if let Some(k) = r.kept
                    && (!eng.core.cont_placements || (r.kids.is_empty() && r.conts.is_empty()))
                {
                    eng.kept.insert(id, (k, r.res.reads.clone(), r.base.unwrap_or_default()));
                }
                eng.enqueue(s, id);
                return Ok(());
            }
            Outcome::Done(r) => match r.res.reads.iter().find(|rd| asked(rd) && !super::build::holds(&eng.core, &eng.w, id, rd)) {
                Some(rd) => {
                    rep.merge.validate += super::core::ns_since(tv);
                    let tr = super::core::now();
                    // A read an earlier commit of this step broke.
                    eng.core.cause(id, super::build::why_broken(&eng.core, &eng.w, id, rd));
                    if !matches!(rd, Read::Range { f, .. } if fold_guess(&eng.core, s, id, *f, true)) {
                        *s.refutes.entry(id).or_default() += 1;
                    }
                    rep.mispredictions += 1;
                    rep.predictions_refuted += u64::from(r.predicted > 0);
                    if let Read::Use { f, .. } | Read::Range { f, .. } | Read::Seq { f, .. } = rd {
                        bump_field(&mut rep.refuted_by_field, *f);
                    }
                    eng.enqueue(s, id);
                    rep.merge.refute += super::core::ns_since(tr);
                    rep.merge.total += super::core::ns_since(tv);
                    rep.merged += 1;
                    return Ok(());
                }
                None => {
                    let v = super::core::ns_since(tv);
                    (rep.merge.validate, rep.merge.total) = (rep.merge.validate + v, rep.merge.total + v);
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
        if let Some((_, why)) = redo {
            let tr = super::core::now();
            eng.core.cause(id, why);
            let w = std::mem::take(&mut eng.w);
            let mut b = Builder::new(&eng.core, w);
            b.make_dirty_for(id, why);
            let m = b.finish();
            m.counts.add_to(rep);
            eng.w = m.w;
            for x in m.dirty {
                eng.enqueue(s, x);
            }
            for x in m.woken {
                eng.wake(s, x);
            }
            let r = super::core::ns_since(tr);
            (rep.merge.refute, rep.merge.total) = (rep.merge.refute + r, rep.merge.total + r);
        }
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
/// Whether workers file their runs' reads (`CLEAN_FILE_AHEAD`, default
/// on).
fn file_ahead() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("CLEAN_FILE_AHEAD").as_deref() != Ok("0"))
}

fn run_job(
    core: &super::core::Core,
    w: &World,
    id: Id,
    resume: Kept,
    predict: bool,
    careful: bool,
    fuel: Option<u64>,
) -> Outcome {
    let (kept, reads, base) = match resume {
        Some((t, r, b)) => (Some(t), r, Some(b)),
        None => (None, Vec::new(), None),
    };
    // Run ahead of a node before it still in work: what it reads may
    // change yet (a guess).
    let ahead = w.node(id).is_some_and(|n| w.work_of(n.unit).is_some_and(|m| m.below(&n.pos).is_some()));
    let t_start = super::core::now();
    let mut cx = Ctx::new(core, w, id, kept);
    cx.reads = reads;
    // (Continuing: the placements its earlier attempts made stand.)
    if core.cont_placements
        && let Some(b) = base
    {
        cx.base = Some(b);
        cx.nkids = b.2;
    }
    cx.predict = predict;
    cx.careful = careful;
    cx.fuel = fuel;
    let op = core.ops[cx.op.0 as usize].clone();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| op.run(&mut cx)));
    if cx.named_gone && r.as_ref().is_ok() {
        drop(cx);
        return run_job(core, w, id, None, predict, careful, fuel);
    }
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
            filed: None,
            def_ix: std::sync::OnceLock::new(),
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
        fold_redo: false,
        span: t_start.zip(super::core::now()),
        base: cx.base,
    };
    match r {
        Ok(Ok(out)) => {
            let mut r = run(cx, out);
            // Its reads filed here, off the merge (`CLEAN_FILE_AHEAD=0`:
            // all by the merge).
            if file_ahead() {
                r.res.filed = super::build::file_ahead(w, id, &r.res.reads);
            }
            Outcome::Done(r)
        }
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

/// Whether a refutation of `id`'s read of accumulator `f` is one of a
/// guessed fold's first two (`Monoid::speculate`): the node runs again
/// guessing, not carefully, its contributors now as they last contributed.
/// `count`: this refutation is counted (else only asked).
fn fold_guess(core: &super::core::Core, s: &mut Sched, id: Id, f: Field, count: bool) -> bool {
    if !core.acc(f).is_some_and(|m| m.speculate()) {
        return false;
    }
    let c = s.fold_refutes.entry(id).or_default();
    *c += u32::from(count);
    *c <= 2
}

/// Whether the steps run a lane (`CLEAN_LANE` set, not to `0`; read once).
fn lane() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CLEAN_LANE").is_some_and(|v| !v.is_empty() && v != "0"))
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
