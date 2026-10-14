//! An evaluation: workers run until the queue is quiet; then edits are
//! absorbed, loop-back rounds advance, and what is still parked is
//! classified (stalled on the environment, a cycle, or no progress).

use std::sync::atomic::Ordering;

use super::edit::Engine;
use super::query::Subj;
use super::{F_MARKED, NIL, S_FREE, S_INERT, S_PARK, Slot, lock, rlock, wlock};
use crate::api::{
    Cause, EditSource, EvalError, Executor, LoopStart, NoEdits, NodeId, Report, Scheduler, Sequential, Settled, WaitOn,
    Waiter,
};
use crate::values::{Field, ValueId};

impl Engine {
    /// Evaluates on the calling thread.
    ///
    /// # Errors
    ///
    /// See [`EvalError`].
    pub fn evaluate_sequential(&mut self) -> Result<Report, EvalError> {
        self.evaluate(&mut crate::api::Greedy::default(), &mut Sequential)
    }

    /// Evaluates with a scheduling policy on an executor.
    ///
    /// # Errors
    ///
    /// See [`EvalError`].
    pub fn evaluate(&mut self, sched: &mut dyn Scheduler, exec: &mut dyn Executor) -> Result<Report, EvalError> {
        self.evaluate_absorbing(sched, exec, &mut NoEdits)
    }

    /// Evaluates, absorbing edits from `src` whenever the work runs out.
    ///
    /// # Errors
    ///
    /// See [`EvalError`].
    pub fn evaluate_absorbing(
        &mut self,
        sched: &mut dyn Scheduler,
        exec: &mut dyn Executor,
        src: &mut dyn EditSource,
    ) -> Result<Report, EvalError> {
        let r = {
            crate::obs::span!(Evaluate);
            self.evaluate_inner(sched, exec, src)
        };
        crate::obs::flush();
        // The next canonical window starts now: edits from here on count
        // toward the next evaluation.
        let sh = &self.sh;
        sh.epoch.fetch_add(1, Ordering::Relaxed);
        sh.c.created.store(0, Ordering::Relaxed);
        sh.c.freed.store(0, Ordering::Relaxed);
        lock(&sh.touched).clear();
        r
    }

    fn evaluate_inner(
        &mut self,
        sched: &mut dyn Scheduler,
        exec: &mut dyn Executor,
        src: &mut dyn EditSource,
    ) -> Result<Report, EvalError> {
        let t0 = std::time::Instant::now();
        self.sh.settle_deferred();
        self.begin(sched);
        let mut rep = Report::default();
        let mut rounds: u64 = 1;
        let mut idle: u64 = 0;
        let mut restarts = 0u64;
        if let Some(v) = src.absorb(self) {
            self.version = v;
            rep.edits_applied.push((v, 0));
        }
        if self.loop_start == LoopStart::Initial && self.sh.qlen.load(Ordering::Acquire) > 0 && self.reset_loops() {
            restarts += 1;
        }
        // An executor that runs every job on one thread evaluates in steps:
        // what a step commits is seen by the next step.
        self.set_stepped(exec.parallelism() <= 1);
        let limits = self.limits().clone();
        *lock(&self.sh.deadline) = limits.time.map(|d| t0 + d);
        let result = loop {
            let before = self.sh.c.runs.load(Ordering::Relaxed) + self.sh.c.commits.load(Ordering::Relaxed);
            // Edits absorbed meanwhile: their frame walks.
            self.sh.settle_deferred();
            self.plan_step(sched);
            {
                let sh = &self.sh;
                // The workers' loops go through `execute`, the hook an
                // executor counts and instruments node runs by.
                let mut done = Vec::with_capacity(64);
                exec.execute(
                    64,
                    &|_| {
                        sh.work();
                        crate::api::RunResult(())
                    },
                    &mut done,
                );
            }
            rep.steps += 1;
            crate::obs::count!(Step);
            self.end_step();
            let err = lock(&self.sh.error).take();
            if let Some(e) = err {
                break Err(self.finish_error(e));
            }
            let after = self.sh.c.runs.load(Ordering::Relaxed) + self.sh.c.commits.load(Ordering::Relaxed);
            if self.sh.qlen.load(Ordering::Acquire) > 0 && self.stepped_between(&mut rep, src, rounds) {
                continue;
            }
            if self.sh.qlen.load(Ordering::Acquire) > 0 {
                if after == before {
                    idle += 1;
                    if idle > u64::from(limits.idle_rounds) {
                        let waiters = self.queued_waiters();
                        break Err(EvalError::NoProgress {
                            rounds: u32::try_from(idle).unwrap_or(u32::MAX),
                            waiters,
                            report: Box::default(),
                        });
                    }
                } else {
                    idle = 0;
                }
                continue;
            }
            idle = 0;
            // Quiet: edits first.
            if let Some(v) = src.absorb(self) {
                self.version = v;
                rep.edits_applied.push((v, rounds));
                self.sh.settle_deferred();
                if self.sh.qlen.load(Ordering::Acquire) > 0 {
                    continue;
                }
            }
            // Loop-back rounds.
            match self.next_round() {
                Err(e) => break Err(e),
                Ok(true) => {
                    rounds += 1;
                    if rounds > self.max_rounds {
                        break Err(EvalError::NonConvergence {
                            rounds: self.max_rounds,
                        });
                    }
                    continue;
                }
                Ok(false) => {}
            }
            // Settled reports, again at each version absorbed with nothing
            // to run.
            let more = loop {
                let items = self.settled_items(&mut rep);
                if !items.is_empty() && !src.settled(self, &items) {
                    self.settled_out.extend(items);
                }
                let Some(v) = src.absorb(self) else { break false };
                self.version = v;
                rep.edits_applied.push((v, rounds));
                self.sh.settle_deferred();
                if self.sh.qlen.load(Ordering::Acquire) > 0 {
                    break true;
                }
            };
            if more {
                continue;
            }
            break Ok(());
        };
        self.sh.abort.store(false, Ordering::Relaxed);
        if let Err(e) = result {
            return Err(self.with_report(e, rep, rounds, restarts));
        }
        let stalled = match self.classify_parked(&mut rep) {
            Ok(s) => s,
            Err(e) => return Err(self.with_report(e, rep, rounds, restarts)),
        };
        if let Some((s, f)) = lock(&self.sh.undeclared).iter().next().map(|(s, f)| (*s, *f))
            && self.sh.rec(s).st() != S_FREE
        {
            return Err(EvalError::UndeclaredDefinition {
                node: self.sh.node_id(s),
                field: f,
            });
        }
        let _ = stalled;
        rep.rounds = rounds;
        rep.restarts = restarts;
        rep.version = self.version;
        if self.sh.c.mispredictions.load(Ordering::Relaxed) > 0 || self.sh.c.speculations.load(Ordering::Relaxed) > 0 {
            self.collect_apps(true);
        }
        self.sh.values.reclaim();
        self.sh.diags.reclaim();
        self.fill_report(&mut rep);
        Ok(rep)
    }

    /// Attaches the counters so far to an error that carries them.
    fn with_report(&mut self, mut e: EvalError, mut rep: Report, rounds: u64, restarts: u64) -> EvalError {
        if let EvalError::NoProgress { report, .. } | EvalError::Deadline { report, .. } = &mut e {
            rep.rounds = rounds;
            rep.restarts = restarts;
            rep.version = self.version;
            self.fill_report(&mut rep);
            **report = rep;
        }
        e
    }

    /// Switches between steps and free running, moving the queue over.
    fn set_stepped(&mut self, on: bool) {
        let sh = &self.sh;
        let mut q = lock(&sh.q);
        sh.stepped.store(on, Ordering::Relaxed);
        if on {
            let cur: Vec<Slot> = std::mem::take(&mut q.cur).into_iter().map(|r| r.0.1).collect();
            q.next.extend(cur);
        }
        Self::start_step(sh, &mut q);
    }

    fn start_step(sh: &super::Shared, q: &mut super::QueueInner) {
        // In steps the scheduler picks each batch (`plan_step`); free
        // running takes everything.
        if !sh.stepped() {
            for s in std::mem::take(&mut q.next) {
                q.cur.push(std::cmp::Reverse((sh.rec(s).lab(), s)));
            }
        }
    }

    /// The next step's batch, as the scheduler picks it from the ready
    /// nodes and (when it speculates) the parked ones that ran before.
    fn plan_step(&mut self, sched: &mut dyn Scheduler) {
        let sh = &self.sh;
        if !sh.stepped() {
            return;
        }
        let mut ready: Vec<Slot> = std::mem::take(&mut lock(&sh.q).next);
        ready.sort_by_key(|&s| (sh.unit_of(s), sh.rec(s).lab()));
        let budget = sched.speculation_budget();
        let mut cands: Vec<Slot> = Vec::new();
        if budget > 0 {
            for &s in lock(&sh.parked).iter() {
                if cands.len() >= budget {
                    break;
                }
                if sh.rec(s).st() == S_PARK {
                    cands.push(s);
                }
            }
        }
        let ids: Vec<NodeId> = ready.iter().map(|&s| sh.node_id(s)).collect();
        let cs: Vec<crate::api::Candidate> = cands
            .iter()
            .map(|&s| crate::api::Candidate {
                node: sh.node_id(s),
                slot: s,
                has_prediction: sh.rec(s).data().res.is_some(),
                waiting: true,
            })
            .collect();
        let mut batch = Vec::new();
        if ready.len() + cs.len() > 0 {
            sched.select(&ids, &cs, &mut batch);
        }
        let pos: crate::FxHashMap<NodeId, usize> = ids.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        let cpos: crate::FxHashMap<NodeId, usize> = cs.iter().enumerate().map(|(i, c)| (c.node, i)).collect();
        let mut taken = vec![false; ready.len()];
        let mut q = lock(&sh.q);
        let mut any_ready = false;
        for j in batch {
            if let Some(&i) = pos.get(&j.node) {
                if taken[i] {
                    continue;
                }
                taken[i] = true;
                any_ready = true;
                let s = ready[i];
                if j.speculative {
                    sh.rec(s).set(super::F_SPECJOB);
                }
                q.cur.push(std::cmp::Reverse((sh.rec(s).lab(), s)));
            } else if let Some(&i) = cpos.get(&j.node) {
                let s = cands[i];
                let r = sh.rec(s);
                r.set(super::F_SPECJOB);
                if r.status
                    .compare_exchange(S_PARK, super::S_DIRTY, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                    && r.flags.fetch_or(super::F_QUEUED, Ordering::AcqRel) & super::F_QUEUED == 0
                {
                    q.cur.push(std::cmp::Reverse((r.lab(), s)));
                    sh.qlen.fetch_add(1, Ordering::Release);
                } else {
                    r.clear(super::F_SPECJOB);
                }
            }
        }
        if !any_ready && !ready.is_empty() {
            // At least one ready node always runs.
            taken[0] = true;
            q.cur.push(std::cmp::Reverse((sh.rec(ready[0]).lab(), ready[0])));
        }
        for (i, s) in ready.into_iter().enumerate() {
            if !taken[i] {
                q.next.push(s);
            }
        }
    }

    /// The end of a step: its commits become clean (visible), the next
    /// step's nodes become ready.
    fn end_step(&mut self) {
        let sh = &self.sh;
        let done = std::mem::take(&mut *lock(&sh.step_done));
        for s in done {
            if sh
                .rec(s)
                .status
                .compare_exchange(super::S_STEP, super::S_CLEAN, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                sh.leave_work_lists(s);
            }
            let w = std::mem::take(&mut sh.rec(s).data().waiters);
            for (n, q) in w {
                sh.wake(n, q);
            }
            sh.guesses_settle(s);
        }
        let mut q = lock(&sh.q);
        Self::start_step(sh, &mut q);
    }

    fn begin(&mut self, sched: &mut dyn Scheduler) {
        let sh = &self.sh;
        // 0: no speculation; 1: on nodes with a prediction; 2: on any.
        // (Free-running workers ask the policy once, with a candidate of
        // each kind; steps ask it for every batch.)
        let spec = if sched.speculation_budget() == 0 {
            0
        } else {
            let probe = [
                crate::api::Candidate {
                    node: NodeId(u64::MAX),
                    slot: u32::MAX,
                    has_prediction: true,
                    waiting: true,
                },
                crate::api::Candidate {
                    node: NodeId(u64::MAX - 1),
                    slot: u32::MAX,
                    has_prediction: false,
                    waiting: true,
                },
            ];
            let mut b = Vec::new();
            sched.select(&[], &probe, &mut b);
            if b.iter().any(|j| j.node == NodeId(u64::MAX - 1)) {
                2
            } else {
                1
            }
        };
        sh.spec.store(spec, Ordering::Relaxed);
        sh.fuel.store(sched.fuel(), Ordering::Relaxed);
        sh.max_outstanding.store(sched.max_outstanding(), Ordering::Relaxed);
        let c = &sh.c;
        for a in [
            &c.runs,
            &c.stops,
            &c.raises,
            &c.pending_waits,
            &c.pending_defining,
            &c.speculations,
            &c.confirmations,
            &c.mispredictions,
            &c.fuel_outs,
            &c.spec_stops,
            &c.resumed,
            &c.restarted,
            &c.declarations_refuted,
            &c.static_contribs_refuted,
            &c.predictions_confirmed,
            &c.predictions_refuted,
            &c.realised,
            &c.env_provided,
            &c.commits,
            &c.scope_region,
            &c.gated,
        ] {
            a.store(0, Ordering::Relaxed);
        }
        *lock(&sh.t) = super::Tallies::default();
        wlock(&sh.env).unknown.clear();
        *lock(&sh.error) = None;
        sh.abort.store(false, Ordering::Relaxed);
        // Parked nodes from an earlier evaluation try again.
        let parked = std::mem::take(&mut *lock(&sh.parked));
        for s in parked {
            if sh.rec(s).st() == S_PARK {
                let seq = sh.rec(s).park_seq.load(Ordering::Acquire);
                sh.wake(s, seq);
            }
        }
    }

    /// Back to the declared initial loop values; whether any changed.
    fn reset_loops(&mut self) -> bool {
        let mut changed = Vec::new();
        {
            let mut lv = wlock(&self.sh.loop_vals);
            for (&f, &init) in &self.sh.loops {
                if lv.get(&f) != Some(&init) {
                    lv.insert(f, init);
                    changed.push(f);
                }
            }
        }
        // The round's end looks at every field again.
        lock(&self.sh.pubs_dirty).extend(self.sh.loops.keys().copied());
        for &f in &changed {
            self.touch_loop_readers(f);
        }
        !changed.is_empty()
    }

    fn touch_loop_readers(&self, f: Field) {
        let readers = lock(&self.sh.loop_readers).remove(&f).unwrap_or_default();
        for (n, g) in readers {
            self.sh.touch(n, g, &Cause::Loop { field: f }, false);
        }
    }

    /// Advances the loop-back fields to their published values; whether
    /// any changed.
    fn next_round(&mut self) -> Result<bool, EvalError> {
        if self.sh.loops.is_empty() {
            return Ok(false);
        }
        // Only the fields whose publications changed since the last round
        // can move; one nobody publishes is back at its initial value.
        let mut fields: Vec<Field> = lock(&self.sh.pubs_dirty).drain().collect();
        fields.sort_by_key(|f| f.0.raw());
        let mut next: Vec<(Field, ValueId)> = Vec::new();
        {
            let pubs = lock(&self.sh.pubs);
            for f in fields {
                let mut vals: Vec<ValueId> = pubs.get(&f).map_or_else(Vec::new, |m| {
                    m.iter()
                        .filter(|(s, _)| self.sh.rec(**s).st() != S_FREE && self.sh.rec(**s).st() != S_INERT)
                        .map(|(_, v)| *v)
                        .collect()
                });
                vals.sort_by_key(|v| v.raw());
                vals.dedup();
                if vals.len() > 1 {
                    // Looked at again at the next round's end.
                    lock(&self.sh.pubs_dirty).insert(f);
                    return Err(EvalError::ConflictingPublication { field: f });
                }
                match vals.first() {
                    Some(&x) => next.push((f, x)),
                    None => {
                        if let Some(&init) = self.sh.loops.get(&f) {
                            next.push((f, init));
                        }
                    }
                }
            }
        }
        let mut changed = Vec::new();
        {
            let mut lv = wlock(&self.sh.loop_vals);
            for (f, x) in next {
                if !self.sh.loops.contains_key(&f) {
                    continue;
                }
                if lv.get(&f) != Some(&x) {
                    lv.insert(f, x);
                    changed.push(f);
                }
            }
        }
        for &f in &changed {
            self.touch_loop_readers(f);
        }
        Ok(!changed.is_empty())
    }

    fn queued_waiters(&self) -> Vec<Waiter> {
        let q = lock(&self.sh.q);
        let mut out: Vec<Waiter> = q
            .cur
            .iter()
            .map(|r| r.0.1)
            .chain(q.next.iter().copied())
            .filter(|&s| self.sh.rec(s).st() != S_FREE)
            .map(|s| Waiter {
                node: self.sh.node_id(s),
                on: WaitOn::Queued,
            })
            .collect();
        out.dedup();
        out
    }

    fn finish_error(&mut self, e: EvalError) -> EvalError {
        match e {
            EvalError::Deadline { runs, .. } => {
                let mut waiters = self.queued_waiters();
                for s in lock(&self.sh.parked).iter() {
                    if self.sh.rec(*s).st() == S_PARK {
                        waiters.push(Waiter {
                            node: self.sh.node_id(*s),
                            on: self.wait_on_of(*s),
                        });
                    }
                }
                EvalError::Deadline {
                    runs,
                    waiters,
                    report: Box::default(),
                }
            }
            e => e,
        }
    }

    fn wait_on_of(&self, s: Slot) -> WaitOn {
        let (on, field) = {
            let d = self.sh.rec(s).data();
            let w = d.wait_on();
            (w.map(|w| w.on.clone()), w.and_then(|w| w.field))
        };
        match on {
            Some(Subj::Node(x)) => WaitOn::Node(self.sh.node_id(x), field),
            Some(Subj::Env(f)) => WaitOn::Environment(f),
            _ => WaitOn::Queued,
        }
    }

    /// Sorts what is still parked: environment stalls (fine), cycles and
    /// no progress (errors).
    fn classify_parked(&mut self, rep: &mut Report) -> Result<u64, EvalError> {
        let mut parked: Vec<Slot> = lock(&self.sh.parked).clone();
        parked.retain(|&s| self.sh.rec(s).st() == S_PARK);
        parked.sort_unstable();
        parked.dedup();
        lock(&self.sh.parked).clone_from(&parked);
        if parked.is_empty() {
            return Ok(0);
        }
        // Follow each node's wait chain.
        let mut fate: crate::values::FxHashMap<Slot, u8> = crate::values::FxHashMap::default(); // 1 env, 2 cycle, 3 other
        let mut cycle: Vec<Slot> = Vec::new();
        for &s in &parked {
            let mut path = Vec::new();
            let mut x = s;
            let verdict = loop {
                if let Some(&v) = fate.get(&x) {
                    break v;
                }
                if path.contains(&x) {
                    // A cycle: through outputs/applications it is an error.
                    let i = path.iter().position(|&p| p == x).unwrap_or(0);
                    let ring = &path[i..];
                    let outputs = ring.iter().all(|&p| {
                        self.sh
                            .rec(p)
                            .data()
                            .wait_on()
                            .is_some_and(|w| matches!(w.kind, "output" | "application"))
                    });
                    let client = self.sh.unit(self.sh.unit_of(s)).app.load(Ordering::Acquire) == NIL;
                    if outputs && client && cycle.is_empty() {
                        cycle = ring.to_vec();
                    }
                    break if outputs { 2 } else { 3 };
                }
                path.push(x);
                if self.sh.rec(x).st() != S_PARK {
                    break 3;
                }
                let w = self.sh.rec(x).data().wait_on().cloned();
                match w.map(|w| w.on) {
                    Some(Subj::Env(f)) => {
                        if !rep.env_waits.iter().any(|e| e.0 == self.sh.node_id(x)) {
                            rep.env_waits.push((self.sh.node_id(x), f));
                        }
                        break 1;
                    }
                    Some(Subj::Node(y)) => x = y,
                    _ => break 3,
                }
            };
            for p in path {
                fate.entry(p).or_insert(verdict);
            }
        }
        if !cycle.is_empty() {
            let ids: Vec<NodeId> = cycle.iter().map(|&s| self.sh.node_id(s)).collect();
            return Err(EvalError::Cycle(ids));
        }
        let env = parked.iter().filter(|s| fate.get(s) == Some(&1)).count() as u64;
        rep.env_stalled = env;
        let others: Vec<Slot> = parked
            .iter()
            .copied()
            .filter(|&s| {
                fate.get(&s) != Some(&1) && self.sh.unit(self.sh.unit_of(s)).app.load(Ordering::Acquire) == NIL
            })
            .collect();
        if !others.is_empty() {
            let waiters = others
                .iter()
                .map(|&s| Waiter {
                    node: self.sh.node_id(s),
                    on: self.wait_on_of(s),
                })
                .collect();
            return Err(EvalError::NoProgress {
                rounds: 1,
                waiters,
                report: Box::default(),
            });
        }
        Ok(env)
    }

    /// Between two steps: absorbs edits, then reports the marked nodes the
    /// evaluation has passed (every entry up to them is clean). `true`: an
    /// edit came in (the step loop goes on).
    fn stepped_between(&mut self, rep: &mut Report, src: &mut dyn EditSource, rounds: u64) -> bool {
        if !self.sh.stepped() {
            return false;
        }
        if let Some(v) = src.absorb(self) {
            self.version = v;
            rep.edits_applied.push((v, rounds));
            self.sh.settle_deferred();
        }
        // Nothing is final while a guess is unconfirmed.
        if lock(&self.sh.marked).is_empty()
            || !self.sh.loops.is_empty()
            || self.sh.outstanding.load(Ordering::Acquire) > 0
        {
            return false;
        }
        let mut items = Vec::new();
        for u in self.units.clone() {
            let es = {
                let _g = rlock(&self.sh.unit(u).s);
                self.sh.entries(u)
            };
            for s in es {
                let r = self.sh.rec(s);
                let st = r.st();
                // An unconfirmed guess ends the settled prefix too.
                if (st != super::S_CLEAN && st != S_INERT && st != super::S_MARKER) || r.has(super::F_GUESS) {
                    break;
                }
                if st == super::S_CLEAN && r.has(F_MARKED) {
                    self.settle_one(s, rep, &mut items);
                }
            }
        }
        rep.settled += items.len() as u64;
        if !items.is_empty() && !src.settled(self, &items) {
            self.settled_out.extend(items);
        }
        false
    }

    fn settle_one(&self, s: Slot, rep: &mut Report, items: &mut Vec<Settled>) {
        let r = self.sh.rec(s);
        let Some(out) = self.output(self.sh.node_id(s)) else {
            return;
        };
        let mut d = r.data();
        let key = (self.version, out);
        if d.x.as_ref().and_then(|e| e.reported) == Some(key) {
            return;
        }
        if d.x
            .as_ref()
            .and_then(|e| e.reported)
            .is_some_and(|k| k.0 == self.version)
        {
            rep.resettled += 1;
        }
        d.ex().reported = Some(key);
        items.push(Settled {
            node: self.sh.node_id(s),
            version: self.version,
            output: out,
        });
    }

    /// Marked nodes final at this version not yet reported.
    fn settled_items(&mut self, rep: &mut Report) -> Vec<Settled> {
        let mut items = Vec::new();
        let sh = &self.sh;
        let mut ms = std::mem::take(&mut *lock(&sh.marked));
        ms.retain(|&s| sh.rec(s).has(F_MARKED) && sh.rec(s).st() != S_FREE);
        ms.sort_by_key(|&s| (sh.unit_of(s), sh.rec(s).lab()));
        ms.dedup();
        for &s in &ms {
            self.settle_one(s, rep, &mut items);
        }
        *lock(&self.sh.marked) = ms;
        rep.settled += items.len() as u64;
        items
    }
}
