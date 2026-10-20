//! An evaluation: workers run until the queue is quiet; then edits are
//! absorbed, loop-back rounds advance, and what is still parked is
//! classified (stalled on the environment, a cycle, or no progress).

use std::sync::atomic::Ordering;

use super::edit::Engine;
use super::query::Subj;
use super::{F_MARKED, NIL, S_FREE, S_INERT, S_PARK, Slot, lock, rlock};
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
        #[cfg(feature = "observe")]
        super::lock(&self.sh.selects).clear();
        #[cfg(feature = "observe")]
        crate::trace::init();
        #[cfg(feature = "observe")]
        let _span = tracing::debug_span!(target: "defuse", "evaluate", version = self.version).entered();
        #[cfg(feature = "observe")]
        let mut rsrc = crate::record::RecSource { inner: src };
        #[cfg(feature = "observe")]
        let src: &mut dyn EditSource = &mut rsrc;
        #[cfg(feature = "observe")]
        crate::record::api(
            &self.sh.values,
            "evaluate_begin",
            serde_json::Value::Null,
            serde_json::Value::Null,
        );
        super::ltrace::init();
        let r = {
            crate::obs::span!(Evaluate);
            #[cfg(feature = "observe")]
            let _watch = super::watch::Watch::start(&self.sh);
            self.evaluate_per_round(sched, exec, src)
        };
        #[cfg(feature = "observe")]
        if let Err(e) = &r
            && matches!(
                e,
                EvalError::NoProgress { .. }
                    | EvalError::Deadline { .. }
                    | EvalError::RunawayNode { .. }
                    | EvalError::RunawayCreation { .. }
            )
        {
            super::watch::emit(&self.sh, &format!("error: {e}"));
        }
        #[cfg(feature = "observe")]
        {
            let r = match &r {
                Ok(rep) => serde_json::json!({"ok": {"nodes_run": rep.nodes_run, "rounds": rep.rounds}}),
                Err(e) => serde_json::json!(e.to_string()),
            };
            tracing::debug!(target: "defuse", event = "evaluate_end", result = %r);
            crate::record::api(&self.sh.values, "evaluate_end", serde_json::Value::Null, r);
        }
        crate::obs::flush();
        if super::ltrace::on() {
            super::ltrace::flush();
        }
        // The next canonical window starts now: edits from here on count
        // toward the next evaluation.
        let sh = &self.sh;
        sh.epoch.fetch_add(1, Ordering::Relaxed);
        sh.c.created.store(0, Ordering::Relaxed);
        sh.c.freed.store(0, Ordering::Relaxed);
        lock(&sh.touched).clear();
        lock(&sh.snaps).clear();
        r
    }

    /// `evaluate_inner`, under `LoopStart::PerRound` first on the quick
    /// path when only values were edited: from the last fixed point, kept
    /// when no change reaches a loop-back publication in any round
    /// (`Shared::quick_holds`: the round sequence stands, the state is the
    /// cold one); otherwise, or when anything else happened, the rounds run
    /// from the initial values with the memo.
    fn evaluate_per_round(
        &mut self,
        sched: &mut dyn Scheduler,
        exec: &mut dyn Executor,
        src: &mut dyn EditSource,
    ) -> Result<Report, EvalError> {
        let memo = &self.sh.memo;
        let mut refused: Option<String> = None;
        if self.loop_start == LoopStart::PerRound && memo.edited.load(Ordering::Acquire) && !memo.quick_eligible() {
            refused = Some(if memo.structural.load(Ordering::Acquire) {
                "an edit the quick path cannot judge (a claimed text, a changed scope effect, claim or finality, push_many)"
            } else {
                "no per-round memo yet"
            }
            .into());
        }
        if self.loop_start == LoopStart::PerRound && memo.quick_eligible() {
            memo.quick.store(true, Ordering::Release);
            memo.edited.store(false, Ordering::Release);
            let r = self.evaluate_inner(sched, exec, src);
            let memo = &self.sh.memo;
            memo.quick.store(false, Ordering::Release);
            let why = match &r {
                Err(_) => Err("the quick path's evaluation failed".to_string()),
                Ok(rep) if rep.rounds != 1 => Err("a loop-back value moved".to_string()),
                Ok(_) if memo.edited.load(Ordering::Acquire) => Err("edits arrived during the evaluation".to_string()),
                Ok(_) => self.sh.quick_holds(),
            };
            let held = why.is_ok();
            refused = why.err();
            if held {
                let sh = &self.sh;
                memo.finish_quick(&|x| sh.rec(x).parent.load(Ordering::Acquire));
                memo.structural.store(false, Ordering::Release);
                let mut rep = r.expect("checked");
                rep.rounds = memo.prev_rounds.load(Ordering::Relaxed);
                rep.capped = lock(&memo.prev_capped).clone();
                rep.memo_bytes = memo.bytes();
                rep.quick_held = true;
                return Ok(rep);
            }
            // The rounds from the start (as after any edit).
            lock(memo.cur_ref()).clear();
            memo.edited.store(true, Ordering::Release);
        }
        let mut r = self.evaluate_inner(sched, exec, src);
        self.sh.memo.structural.store(false, Ordering::Release);
        if let Ok(rep) = &mut r {
            rep.quick_refused = refused;
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
        self.sh.settle_deferred();
        self.sh.reset_delta();
        self.begin(sched);
        let mut rep = Report::default();
        let mut rounds: u64 = 1;
        // Per-round times: the counters at the round's start.
        let snap = |sh: &super::Shared| {
            (
                std::time::Instant::now(),
                sh.c.op_ns.load(Ordering::Relaxed),
                sh.c.commit_ns.load(Ordering::Relaxed),
                sh.c.check_ns.load(Ordering::Relaxed),
                sh.c.idle_ns.load(Ordering::Relaxed),
                sh.c.runs.load(Ordering::Relaxed),
                sh.memo.hits.load(Ordering::Relaxed),
            )
        };
        let close =
            |sh: &super::Shared, m: &(std::time::Instant, u64, u64, u64, u64, u64, u64)| crate::api::RoundTimes {
                wall_ns: u64::try_from(m.0.elapsed().as_nanos()).unwrap_or(u64::MAX),
                op_ns: sh.c.op_ns.load(Ordering::Relaxed) - m.1,
                commit_ns: sh.c.commit_ns.load(Ordering::Relaxed) - m.2,
                check_ns: sh.c.check_ns.load(Ordering::Relaxed) - m.3,
                idle_ns: sh.c.idle_ns.load(Ordering::Relaxed) - m.4,
                runs: sh.c.runs.load(Ordering::Relaxed) - m.5,
                memo_hits: sh.memo.hits.load(Ordering::Relaxed) - m.6,
            };
        let mut mark = snap(&self.sh);
        let mut idle: u64 = 0;
        // Fields still moving when the round cap stopped the rounds.
        let mut capped: Option<Vec<Field>> = None;
        let mut restarts = 0u64;
        if let Some(v) = src.absorb(self) {
            self.version = v;
            rep.edits_applied.push((v, 0));
        }
        // PerRound: after any edit the rounds start from the initial
        // values, as a fresh engine's do.
        // Initial and PerRound: after any edit the rounds start from the
        // initial values, as a fresh engine's do (the quick path aside).
        let quick = self.sh.memo.quick.load(Ordering::Acquire);
        let edited = !quick && self.sh.memo.edited.swap(false, Ordering::AcqRel);
        let from_start = matches!(self.loop_start, LoopStart::Initial | LoopStart::PerRound);
        if from_start && !quick && (edited || self.sh.qlen.load(Ordering::Acquire) > 0) && self.reset_loops() {
            restarts += 1;
        }
        // An executor that runs every job on one thread may ask for steps
        // (`Executor::steps`): what a step commits is seen by the next.
        self.set_stepped(exec.parallelism() <= 1 && exec.steps());
        let limits = self.limits().clone();
        self.sh.deadline = limits.time.map(|d| t0 + d);
        let result = loop {
            let before = self.sh.c.runs.load(Ordering::Relaxed) + self.sh.c.commits.load(Ordering::Relaxed);
            // Edits absorbed meanwhile: their frame walks.
            self.sh.settle_deferred();
            #[cfg(feature = "observe")]
            let _step = tracing::debug_span!(target: "defuse", "step", n = rep.steps).entered();
            self.plan_step(sched);
            {
                let sh = &self.sh;
                // The workers' loops go through `execute`, the hook an
                // executor counts and instruments node runs by.
                let mut done = Vec::with_capacity(64);
                #[cfg(feature = "profile")]
                let tx = std::time::Instant::now();
                #[cfg(feature = "profile")]
                let _tx = crate::obs::Ticked("exec_wall", tx);
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
            #[cfg(feature = "profile")]
            let te = std::time::Instant::now();
            self.end_step();
            #[cfg(feature = "profile")]
            #[allow(clippy::cast_possible_truncation)]
            crate::obs::add_impl("end_step", te.elapsed().as_nanos() as u64);
            let err = lock(&self.sh.error).take();
            if let Some(e) = err {
                // (A hook's panic is reported once.)
                let mut h = lock(&self.sh.hook_error);
                if h.as_ref() == Some(&e) {
                    *h = None;
                }
                drop(h);
                break Err(self.finish_error(e));
            }
            let after = self.sh.c.runs.load(Ordering::Relaxed) + self.sh.c.commits.load(Ordering::Relaxed);
            // Loop-back fields runs declared: in effect now, within this
            // evaluation (their readers that bound them otherwise run again).
            if self.sh.qlen.load(Ordering::Acquire) == 0 {
                let decl = std::mem::take(&mut *lock(&self.sh.pending_loops));
                if !decl.is_empty() {
                    for (f, init) in decl {
                        self.declare_loopback_inner(f, init);
                    }
                    if self.sh.qlen.load(Ordering::Acquire) > 0 {
                        continue;
                    }
                }
            }
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
            // Loop-back rounds (none past a cap: the final phase runs on
            // the values the last round ran with).
            let before_round = (self.cap_policy == crate::api::CapPolicy::Finalize && capped.is_none())
                .then(|| self.sh.loop_vals.read().clone());
            let next = if capped.is_some() {
                Ok(Vec::new())
            } else {
                self.next_round()
            };
            match next {
                Err(e) => break Err(e),
                // The rounds had converged: a final node moved a loop-back.
                Ok(ch) if !ch.is_empty() && self.sh.final_phase.load(Ordering::Acquire) => {
                    break Err(EvalError::FinalMovedLoop {
                        node: self.final_mover(ch[0]),
                        field: ch[0],
                    });
                }
                // (A round only when a changed value has a reader to run,
                // final nodes aside.)
                Ok(ch) if !ch.is_empty() && !self.round_reader => {
                    self.sh.memo.note_change(ch, false, Vec::new());
                    continue;
                }
                Ok(ch) if !ch.is_empty() => {
                    self.sh
                        .memo
                        .note_change(ch.clone(), true, std::mem::take(&mut self.round_readers));
                    rep.round_times.push(close(&self.sh, &mark));
                    mark = snap(&self.sh);
                    rounds += 1;
                    self.sh.memo.next_round(u32::try_from(rounds).unwrap_or(u32::MAX - 1));
                    #[cfg(feature = "observe")]
                    tracing::debug!(target: "defuse", event = "round", round = rounds);
                    if rounds > self.max_rounds {
                        let Some(vals) = before_round else {
                            break Err(EvalError::NonConvergence {
                                rounds: self.max_rounds,
                            });
                        };
                        // Finalize: back to the last round's values (its
                        // readers, touched for the next one, run again on
                        // them and change nothing); the final phase next.
                        *self.sh.loop_vals.write() = vals;
                        rounds = self.max_rounds;
                        self.sh.memo.next_round(u32::try_from(rounds).unwrap_or(u32::MAX - 1));
                        capped = Some(ch);
                    }
                    continue;
                }
                Ok(_) => {}
            }
            // Converged: the final phase runs the held final nodes, then
            // the rounds are asked once more.
            if self.sh.any_final.load(Ordering::Relaxed)
                && !self.sh.final_phase.load(Ordering::Acquire)
                && !lock(&self.sh.held).is_empty()
            {
                self.sh.final_phase.store(true, Ordering::Release);
                self.sh.memo.next_round(super::memo::FINAL);
                self.sh.release_final();
                continue;
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
        #[cfg(feature = "profile")]
        let tail = std::time::Instant::now();
        #[cfg(feature = "profile")]
        #[allow(clippy::cast_possible_truncation)]
        let tick = |name: &'static str, t: &mut std::time::Instant| {
            crate::obs::add_impl(name, t.elapsed().as_nanos() as u64);
            *t = std::time::Instant::now();
        };
        #[cfg(feature = "profile")]
        let mut tt = tail;
        let quick = self.sh.memo.quick.load(Ordering::Acquire);
        if let Err(e) = result {
            // (A quick path that failed falls back to the rounds, memo kept.)
            if !quick {
                self.sh.memo.drop_all();
            }
            return Err(self.with_report(e, rep, rounds, restarts));
        }
        if self.sh.memo.on.load(Ordering::Relaxed) && !quick {
            let sh = &self.sh;
            sh.memo
                .finish(rounds, capped.clone(), &|x| sh.rec(x).parent.load(Ordering::Acquire));
            rep.memo_bytes = self.sh.memo.bytes();
        }
        if self.sh.memo.on.load(Ordering::Relaxed) {
            let m = std::mem::take(&mut *lock(&self.sh.memo.misses));
            rep.memo_misses = m
                .into_iter()
                .map(|(id, round, why)| crate::api::MemoMiss {
                    node: NodeId(id),
                    round,
                    why,
                })
                .collect();
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
        #[cfg(feature = "profile")]
        tick("tail_classify", &mut tt);
        rep.round_times.push(close(&self.sh, &mark));
        rep.rounds = rounds;
        rep.restarts = restarts;
        rep.final_nodes_run = self.sh.final_released.load(Ordering::Relaxed);
        rep.capped = capped;
        rep.version = self.version;
        if self.sh.c.mispredictions.load(Ordering::Relaxed) > 0 || self.sh.c.speculations.load(Ordering::Relaxed) > 0 {
            self.collect_apps(true);
        }
        #[cfg(feature = "profile")]
        tick("tail_collect", &mut tt);
        self.sh.values.reclaim();
        self.sh.diags.reclaim();
        #[cfg(feature = "profile")]
        tick("tail_reclaim", &mut tt);
        if std::env::var_os("DEFUSE_MERGESTAT").is_some() {
            let c = &self.sh.c;
            eprintln!(
                "merges {} items {} merge {:.1} ms",
                c.merges.load(Ordering::Relaxed),
                c.merged.load(Ordering::Relaxed),
                c.merge_ns.load(Ordering::Relaxed) as f64 / 1e6
            );
        }
        self.fill_report(&mut rep, Some(exec));
        #[cfg(feature = "profile")]
        tick("tail_report", &mut tt);
        Ok(rep)
    }

    /// Attaches the counters so far to an error that carries them.
    fn with_report(&mut self, mut e: EvalError, mut rep: Report, rounds: u64, restarts: u64) -> EvalError {
        if let EvalError::NoProgress { report, .. } | EvalError::Deadline { report, .. } = &mut e {
            rep.rounds = rounds;
            rep.restarts = restarts;
            rep.version = self.version;
            self.fill_report(&mut rep, None);
            rep.state_dump = Some(self.sh.dump());
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
            for m in &sh.shards {
                q.next
                    .extend(std::mem::take(&mut *lock(&m.h)).into_iter().map(|r| r.0.1));
                m.top.store(u64::MAX, Ordering::Release);
            }
        }
        Self::start_step(sh, &mut q);
    }

    fn start_step(sh: &super::Shared, q: &mut super::QueueInner) {
        // In steps the scheduler picks each batch (`plan_step`); free
        // running takes everything.
        if !sh.stepped() {
            let cur: Vec<Slot> = std::mem::take(&mut q.cur).into_iter().map(|r| r.0.1).collect();
            for s in std::mem::take(&mut q.next).into_iter().chain(cur) {
                sh.push_shard(s);
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
                // (Not one waiting on the environment: nothing it can
                // guess changes until the environment does.)
                if sh.rec(s).st() == S_PARK
                    && !matches!(
                        sh.rec(s).data().wait_on().map(|w| &w.on),
                        Some(super::query::Subj::Env(_))
                    )
                {
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
                    sh.work.fetch_add(1, Ordering::SeqCst);
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
        sh.merge_held();
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
        sh.final_phase.store(false, Ordering::Release);
        let per_round = self.loop_start == LoopStart::PerRound;
        if !per_round && sh.memo.on.load(Ordering::Relaxed) {
            sh.memo.drop_all();
        }
        sh.memo.on.store(per_round, Ordering::Release);
        // (The quick path records under the last evaluation's last round.)
        let first = if sh.memo.quick.load(Ordering::Acquire) {
            u32::try_from(sh.memo.prev_rounds.load(Ordering::Relaxed)).unwrap_or(1)
        } else {
            1
        };
        sh.memo.next_round(first);
        sh.memo.hits.store(0, Ordering::Relaxed);
        sh.memo.begin();
        sh.final_released.store(0, Ordering::Relaxed);
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
            &c.op_ns,
            &c.commit_ns,
            &c.check_ns,
            &c.idle_ns,
            &c.claims_refuted,
            &c.defs_refuted,
            &c.effects_refuted,
            &c.claim_siblings,
            &c.sole_successor,
            &c.many_successors,
            &c.kept_successors,
            &c.resumed,
            &c.restarted,
            &c.declarations_refuted,
            &c.static_contribs_refuted,
            &c.predictions_confirmed,
            &c.predictions_refuted,
            &c.nodes_conservative,
            &c.stable_scans,
            &c.env_provided,
            &c.commits,
            &c.scope_region,
            &c.gated,
        ] {
            a.store(0, Ordering::Relaxed);
        }
        for a in &c.read_ns {
            a.store(0, Ordering::Relaxed);
        }
        c.realised.store(0, Ordering::Relaxed);
        c.realised_ns.store(0, Ordering::Relaxed);
        for t in &*sh.t {
            *lock(&t.0) = super::Tallies::default();
        }
        {
            // Refutations carry over, fading by one an evaluation: a field
            // refuted in the last edit is not guessed in the next ones, and
            // guessed again (a probe) once its count fades.
            let mut m = sh.no_guess.write();
            m.retain(|_, n| {
                *n = n.saturating_sub(1);
                *n > 0
            });
            let any = m.values().any(|&n| n >= super::NO_GUESS_AFTER);
            sh.any_no_guess.store(any, Ordering::Relaxed);
        }
        sh.env.write().unknown.clear();
        *lock(&sh.error) = None;
        sh.abort.store(false, Ordering::Relaxed);
        if let Some(e) = lock(&sh.hook_error).take() {
            sh.fail(e);
        }
        // Parked nodes from an earlier evaluation try again, unless what
        // each waits on is a node still in work: that node's commit wakes
        // it, and a change to what it read touches it (a node still parked
        // was touched by nothing since it parked). Re-checking every such
        // wait each evaluation cost a pass over their kept logs.
        let parked = std::mem::take(&mut *lock(&sh.parked));
        let mut still = Vec::new();
        for s in parked {
            if sh.rec(s).st() != S_PARK {
                continue;
            }
            let on = sh.rec(s).data().wait_on().map(|w| w.on.clone());
            if let Some(super::query::Subj::Node(x)) = on
                && super::in_work(sh.rec(x).st())
            {
                still.push(s);
                continue;
            }
            let seq = sh.rec(s).park_seq.load(Ordering::Acquire);
            sh.wake(s, seq);
        }
        lock(&sh.parked).extend(still);
    }

    /// Back to the declared initial loop values; whether any changed.
    fn reset_loops(&mut self) -> bool {
        let mut changed = Vec::new();
        {
            let mut lv = self.sh.loop_vals.write();
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
            let _ = self.touch_loop_readers(f);
        }
        !changed.is_empty()
    }

    /// Touches the readers of loop-back `f`; whether one is not a final
    /// node (a final node's read, made in the final phase, does not make a
    /// round: a cold build's final nodes have not run when rounds turn).
    fn touch_loop_readers(&mut self, f: Field) -> bool {
        let readers = lock(&self.sh.loop_readers).remove(&f).unwrap_or_default();
        let mut live = false;
        let mut live_ids = Vec::new();
        // A reader stays subscribed while its log is current: touched, it
        // may be confirmed without a commit (its value came back), and a
        // later change must reach it still.
        let mut keep = Vec::new();
        for (n, g) in readers {
            let r = self.sh.rec(n);
            let current = r.st() != S_FREE && r.lgen.load(Ordering::Acquire) == g;
            if current {
                keep.push((n, g));
                // (Only a clean node's read is this round's: one still in
                // work, parked behind a held final node say, read it under
                // an earlier text or round.)
                if r.st() == super::S_CLEAN && !r.data().stat.fin() {
                    live = true;
                    live_ids.push(self.sh.node_id(n).0);
                }
            }
            self.sh.touch(n, g, &Cause::Loop { field: f }, false);
        }
        if !keep.is_empty() {
            lock(&self.sh.loop_readers).entry(f).or_default().extend(keep);
        }
        self.round_readers.extend(live_ids);
        live
    }

    /// Advances the loop-back fields to their published values; the ones
    /// that changed.
    fn next_round(&mut self) -> Result<Vec<Field>, EvalError> {
        if self.sh.loops.is_empty() {
            return Ok(Vec::new());
        }
        // Every loop-back field is looked at (few; a field left out of the
        // dirty set after an error or a reset would keep a stale value):
        // one nobody publishes is back at its initial value.
        lock(&self.sh.pubs_dirty).clear();
        let mut fields: Vec<Field> = self.sh.loops.keys().copied().collect();
        fields.sort_by_key(|f| f.0.raw());
        let mut next: Vec<(Field, ValueId)> = Vec::new();
        {
            let pubs = lock(&self.sh.pubs);
            for f in fields {
                let mut vals: Vec<ValueId> = pubs.get(&f).map_or_else(Vec::new, |m| {
                    m.iter()
                        // Only a node whose committed result is current: one
                        // still in work (a final node held unrun, a reader
                        // parked behind it, a text not yet run) published
                        // that under an earlier text or round; its next
                        // commit replaces it, and the next round looks again.
                        .filter(|(s, _)| self.sh.rec(**s).st() == super::S_CLEAN)
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
            let mut lv = self.sh.loop_vals.write();
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
        let mut live = false;
        self.round_readers.clear();
        for &f in &changed {
            live |= self.touch_loop_readers(f);
        }
        self.round_reader = live;
        Ok(changed)
    }

    /// The node whose publication moved `f` in the final phase: a final
    /// node, else a node that ran after them (it read one); first by id.
    fn final_mover(&self, f: Field) -> NodeId {
        let sh = &self.sh;
        let live = |s: &Slot| sh.rec(*s).st() != S_FREE;
        let pubs = lock(&sh.pubs);
        let mut cands: Vec<Slot> = pubs
            .get(&f)
            .map(|m| m.keys().copied().filter(live).collect())
            .unwrap_or_default();
        drop(pubs);
        cands.sort_unstable_by_key(|&s| (!sh.rec(s).data().stat.fin(), sh.node_id(s).0));
        cands.first().map_or(NodeId(0), |&s| sh.node_id(s))
    }

    fn queued_waiters(&self) -> Vec<Waiter> {
        let q = lock(&self.sh.q);
        let sharded: Vec<Slot> = self
            .sh
            .shards
            .iter()
            .flat_map(|m| lock(&m.h).iter().map(|r| r.0.1).collect::<Vec<_>>())
            .collect();
        let mut out: Vec<Waiter> = q
            .cur
            .iter()
            .map(|r| r.0.1)
            .chain(q.next.iter().copied())
            .chain(sharded)
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
        // The same nodes parked on the same parks as last time: the same
        // waits, the same verdicts (a stall that never moves is classified
        // once, not on every evaluation).
        let key: Vec<(Slot, u32)> = parked
            .iter()
            .map(|&s| (s, self.sh.rec(s).park_seq.load(Ordering::Acquire)))
            .collect();
        if let Some((k, waits, env)) = &self.parked_memo
            && *k == key
        {
            for w in waits {
                if !rep.env_waits.contains(w) {
                    rep.env_waits.push(*w);
                }
            }
            rep.env_stalled = *env;
            return Ok(*env);
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
        self.parked_memo = Some((key, rep.env_waits.clone(), env));
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
        let cands = self.report_candidates();
        if cands.is_empty() {
            return false;
        }
        // Per unit, the settled prefix up to its last candidate (in order).
        let mut items = Vec::new();
        let mut left = Vec::new();
        let mut i = 0;
        while i < cands.len() {
            let u = self.sh.unit_of(cands[i]);
            let mut j = i;
            while j < cands.len() && self.sh.unit_of(cands[j]) == u {
                j += 1;
            }
            let want: crate::FxHashSet<Slot> = cands[i..j].iter().copied().collect();
            let mut found = 0;
            let mut reached = crate::FxHashSet::default();
            {
                let _g = rlock(&self.sh.unit(u).s);
                let mut x = self.sh.unit(u).first.load(Ordering::Acquire);
                while x != NIL && found < want.len() {
                    let r = self.sh.rec(x);
                    let st = r.st();
                    // An unconfirmed guess ends the settled prefix too.
                    if (st != super::S_CLEAN && st != S_INERT && st != super::S_MARKER) || r.has(super::F_GUESS) {
                        break;
                    }
                    if want.contains(&x) {
                        found += 1;
                        if st == super::S_CLEAN {
                            self.settle_one(x, rep, &mut items);
                            reached.insert(x);
                        }
                    }
                    x = self.sh.next(x);
                }
            }
            left.extend(cands[i..j].iter().copied().filter(|c| !reached.contains(c)));
            i = j;
        }
        lock(&self.sh.to_report).extend(left);
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
        for s in self.report_candidates() {
            self.settle_one(s, rep, &mut items);
        }
        rep.settled += items.len() as u64;
        items
    }

    /// The marked nodes a report may be due for, in order: those that
    /// committed or were marked since, and every marked node at a new
    /// version.
    fn report_candidates(&mut self) -> Vec<Slot> {
        let sh = &self.sh;
        let mut c = std::mem::take(&mut *lock(&sh.to_report));
        if self.report_version != Some(self.version) {
            self.report_version = Some(self.version);
            let mut ms = lock(&sh.marked);
            ms.retain(|&s| sh.rec(s).has(F_MARKED) && sh.rec(s).st() != S_FREE);
            ms.sort_unstable();
            ms.dedup();
            c.extend(ms.iter().copied());
        }
        c.retain(|&s| sh.rec(s).has(F_MARKED) && sh.rec(s).st() != S_FREE);
        c.sort_by_key(|&s| (sh.unit_of(s), sh.rec(s).lab(), s));
        c.dedup();
        c
    }
}
