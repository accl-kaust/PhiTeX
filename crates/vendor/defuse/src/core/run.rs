//! The worker: takes a node off the queue, validates its committed reads,
//! runs it when one broke, and commits or parks it on the worker that ran
//! it. No barrier: a worker stops when the queue is empty and nobody is
//! busy.

use std::sync::atomic::Ordering;

use super::ctx::Ctx;
use super::order::Notes;
use super::query::{Subj, Wait};
use super::{
    F_DOOM_FREE, F_DOOM_INERT, F_NEEDS_RUN, F_QUEUED, F_STALE, Kept, NIL, Pred, S_DIRTY, S_FREE, S_NEW, S_PARK, S_RUN,
    Shared, Slot, WaitRec, in_work, lock, rlock, wlock,
};
use crate::api::{Cause, EvalError};

/// What validating a log found.
pub(crate) enum Check {
    Holds,
    Broke(&'static str),
    Wait(Wait),
}

impl Shared {
    fn pop(&self) -> Option<Slot> {
        let mut q = lock(&self.q);
        let std::cmp::Reverse((_, s)) = q.cur.pop()?;
        self.busy.fetch_add(1, Ordering::SeqCst);
        self.qlen.fetch_sub(1, Ordering::SeqCst);
        Some(s)
    }

    /// Whether the evaluation is quiescent: nothing queued, nobody busy.
    fn quiet(&self) -> bool {
        let q = lock(&self.q);
        q.cur.is_empty() && self.busy.load(Ordering::SeqCst) == 0
    }

    /// One worker: processes nodes until the evaluation is quiescent.
    pub(crate) fn work(&self) {
        let mut idle = 0u32;
        loop {
            if self.abort.load(Ordering::Relaxed) {
                return;
            }
            if let Some(s) = self.pop() {
                idle = 0;
                {
                    crate::obs::span!(RunBatch);
                    self.process(s);
                }
                if self.busy.fetch_sub(1, Ordering::SeqCst) == 1 && self.qlen.load(Ordering::SeqCst) == 0 {
                    // Perhaps quiescent: the sleepers look.
                    self.wake_idle(true);
                }
                continue;
            }
            if self.quiet() {
                return;
            }
            idle += 1;
            if idle == 2_000_000 {
                let q = lock(&self.q);
                eprintln!(
                    "STUCK qlen {} busy {} cur {} next {}",
                    self.qlen.load(Ordering::SeqCst),
                    self.busy.load(Ordering::SeqCst),
                    q.cur.len(),
                    q.next.len()
                );
            }
            if idle < 64 {
                std::hint::spin_loop();
            } else if idle < 4096 {
                std::thread::yield_now();
            } else {
                // Asleep until work is queued or the evaluation ends (a
                // bounded wait: a missed wake costs a millisecond).
                let g = lock(&self.idle_mx);
                self.sleepers.fetch_add(1, Ordering::SeqCst);
                if self.qlen.load(Ordering::SeqCst) == 0 && !self.abort.load(Ordering::Relaxed) {
                    let _ = self.idle_cv.wait_timeout(g, std::time::Duration::from_millis(1));
                }
                self.sleepers.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    /// Validates a recorded log in the strict view.
    pub(crate) fn check_log(&self, me: Slot, log: &[Pred]) -> Check {
        let u = self.unit_of(me);
        self.sread(u, || self.check_log_in(me, log))
    }

    #[inline(never)]
    pub(crate) fn check_log_in(&self, me: Slot, log: &[Pred]) -> Check {
        for p in log {
            match self.recheck(me, p, true) {
                Err(w) => return Check::Wait(w),
                Ok(Err(why)) => return Check::Broke(why),
                Ok(Ok(())) => {}
            }
        }
        Check::Holds
    }

    /// An in-work pending claimer before `s` whose claim may cover it.
    pub(crate) fn held_by(&self, s: Slot) -> Option<Slot> {
        let u = self.unit_of(s);
        self.sread(u, || self.held_by_in(s))
    }

    #[inline(never)]
    pub(crate) fn held_by_in(&self, s: Slot) -> Option<Slot> {
        let u = self.unit_of(s);
        let list = super::rlock(&self.unit(u).pclaim);
        if list.is_empty() {
            return None;
        }
        let ls = self.rec(s).lab();
        for &c in list.iter().rev() {
            if c == s || self.rec(c).lab() >= ls || !in_work(self.rec(c).st()) || !self.rec(c).has(super::F_IN_PCLAIM) {
                continue;
            }
            let limit = self.rec(c).data().stat.limit;
            // `s` is in `c`'s following view only if its parent is an
            // ancestor-or-self of `c`'s parent.
            let ps = self.rec(s).parent.load(Ordering::Acquire);
            let mut a = self.rec(c).parent.load(Ordering::Acquire);
            let mut related = a == ps;
            while !related && a != NIL {
                a = self.rec(a).parent.load(Ordering::Acquire);
                related = a == ps;
            }
            if !related {
                continue;
            }
            // A delimiter by its text alone is never inside the claim: it
            // ends it (the claimer's walk stops there in either view).
            {
                let (cop, cargs) = self.text(c);
                let (sop, sargs) = self.text(s);
                if matches!(
                    self.ops[cop.0 as usize].until(&cargs, &self.values, sop, &sargs),
                    crate::api::Until::Yes
                ) {
                    continue;
                }
            }
            let hit = match limit {
                None => true,
                Some(limit) => {
                    let mut k = 0usize;
                    let mut hit = false;
                    self.following(c, c, |x, skipped| {
                        if self.rec(x).lab() > ls {
                            return false;
                        }
                        if x == s {
                            hit = !skipped && k < limit;
                            return false;
                        }
                        if !skipped {
                            k += 1;
                        }
                        k < limit
                    });
                    hit
                }
            };
            if hit {
                return Some(c);
            }
        }
        None
    }

    /// Takes node `s` through validation, a run and its commit, letting
    /// it guess if the policy chose it for that.
    pub(crate) fn process(&self, s: Slot) {
        let r = self.rec(s);
        let mode = self.spec.load(Ordering::Relaxed);
        // A job the scheduler chose speculatively always may guess; any
        // other run guesses on a definer in work as the mode allows.
        let chosen = r.flags.fetch_and(!super::F_SPECJOB, Ordering::AcqRel) & super::F_SPECJOB != 0;
        let spec = chosen || mode == 2 || (mode == 1 && r.data().res.is_some());
        self.set_spec_run(spec && mode != 0);
        self.process_job(s);
        self.set_spec_run(false);
    }

    fn process_job(&self, s: Slot) {
        let r = self.rec(s);
        r.clear(F_QUEUED);
        let st = loop {
            let st = r.st();
            if st != S_NEW && st != S_DIRTY {
                return;
            }
            if r.status
                .compare_exchange(st, S_RUN, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break st;
            }
        };
        if self.doomed(s) {
            return;
        }
        if let Some(c) = self.held_by(s) {
            if self.rec(c).st() == super::S_STEP {
                self.defer_step(s, "claim");
                return;
            }
            let w = Wait {
                on: Subj::Node(c),
                field: None,
                prefix: None,
                kind: "claim",
                has_prev: false,
            };
            // Held before it runs: gated, not stopped. One with a
            // committed log it has not been told to drop validates it
            // when woken.
            let run = r.has(F_NEEDS_RUN) || r.data().res.is_none();
            let kept = r.data().kept.take();
            self.park_with(s, w, kept, 1, run);
            return;
        }
        let has_res = r.data().res.is_some();
        let kept_log = r.data().kept.as_ref().map(|k| k.log.clone());
        if let Some(log) = kept_log {
            match self.check_log(s, &log) {
                Check::Wait(w) => {
                    let token = r.data().kept.take().and_then(|k| k.token);
                    self.park(s, w, Some(Kept { log, token }), 0);
                    return;
                }
                Check::Broke(why) => {
                    r.data().kept = None;
                    self.c.restarted.fetch_add(1, Ordering::Relaxed);
                    *lock(&self.t).restarted_by.entry(why).or_default() += 1;
                }
                Check::Holds => {
                    // Still waiting where it stopped: park again unrun.
                    let ask = r.data().x.as_ref().and_then(|e| e.ask);
                    if let Some(w) = ask.and_then(|a| self.reask(s, a)) {
                        let kept = r.data().kept.take();
                        self.park(s, w, kept, 1);
                        return;
                    }
                    self.c.resumed.fetch_add(1, Ordering::Relaxed);
                }
            }
        } else if st == S_DIRTY && has_res && !r.has(F_NEEDS_RUN) {
            let log = r.data().res.as_ref().map_or_else(Default::default, |c| c.log.clone());
            r.set(super::F_CHECKING);
            let ck = self.check_log(s, &log);
            match ck {
                Check::Holds => {
                    let done = self.settle_clean(s);
                    r.clear(super::F_CHECKING);
                    if done {
                        return;
                    }
                }
                Check::Wait(w) => {
                    // Validation parks with nothing kept: it re-validates.
                    r.clear(super::F_CHECKING);
                    self.park_validation(s, w);
                    return;
                }
                Check::Broke(_) => {
                    r.clear(super::F_CHECKING);
                    if r.has(super::F_PREDREAD | super::F_PROJREAD) {
                        let f = self.broken_field(s, &log);
                        self.prediction_broke(s, f);
                    }
                }
            }
        }
        if let Some(w) = self.gate(s) {
            if let Subj::Node(x) = w.on
                && self.rec(x).st() == super::S_STEP
            {
                self.defer_step(s, "gated");
                return;
            }
            let kept = r.data().kept.take();
            self.park(s, w, kept, 1);
            return;
        }
        self.run_node(s);
    }

    /// The field of the first predicted read in `log` that no longer
    /// holds.
    fn broken_field(&self, s: Slot, log: &[super::Pred]) -> Option<crate::values::Field> {
        log.iter().find_map(|p| match p {
            super::Pred::Def { f, spec: Some(_), .. }
                if matches!(self.check_log(s, std::slice::from_ref(p)), Check::Broke(_)) =>
            {
                Some(*f)
            }
            _ => None,
        })
    }

    pub(crate) fn reask(&self, s: Slot, a: super::Ask) -> Option<Wait> {
        let u = self.unit_of(s);
        self.sread(u, || self.reask_in(s, a))
    }

    fn reask_in(&self, s: Slot, a: super::Ask) -> Option<Wait> {
        match a {
            super::Ask::Def(f) => match self.q_def(s, f, true, s) {
                super::query::Q::Wait(w) => Some(w),
                super::query::Q::Ans(_) => None,
            },
            super::Ask::Range(f, from, to) => match self.q_range(s, f, from, to, true, s) {
                super::query::Q::Wait(w) => Some(w),
                super::query::Q::Ans(_) => None,
            },
        }
    }

    /// A node whose read hint names a field still in work before it waits
    /// without running.
    pub(crate) fn gate(&self, s: Slot) -> Option<Wait> {
        let u = self.unit_of(s);
        self.sread(u, || self.gate_in(s))
    }

    fn gate_in(&self, s: Slot) -> Option<Wait> {
        // The hint is the text's: analysed once, not per attempt.
        let fields = self.rec(s).data().stat.reads.clone()?;
        for f in fields {
            if rlock(&self.loop_vals).contains_key(&f) {
                continue;
            }
            if let super::query::Q::Wait(mut w) = self.q_def(s, f, true, s) {
                w.kind = "gated";
                return Some(w);
            }
        }
        None
    }

    /// Held by a node that committed in this step: it runs in the next
    /// step, where that commit is seen; no park and wake for it.
    fn defer_step(&self, s: Slot, kind: &'static str) {
        let r = self.rec(s);
        if !self.leave_run(s, S_DIRTY) {
            return;
        }
        crate::obs::count!(Gated);
        self.c.gated.fetch_add(1, Ordering::Relaxed);
        *lock(&self.t).waits_by_kind.entry(kind).or_default() += 1;
        let _ = r;
        self.enqueue(s);
    }

    /// Leaves the running state for `to`; `false` when the node was doomed
    /// meanwhile (it is freed or made inert here, or by whoever doomed it).
    pub(crate) fn leave_run(&self, s: Slot, to: u8) -> bool {
        let r = self.rec(s);
        if r.status
            .compare_exchange(S_RUN, to, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        if r.has(F_DOOM_FREE | F_DOOM_INERT) {
            if r.status
                .compare_exchange(to, S_RUN, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.doomed(s);
            }
            return false;
        }
        true
    }

    /// Frees or deactivates a node whose worker was told to.
    pub(crate) fn doomed(&self, s: Slot) -> bool {
        let r = self.rec(s);
        if !r.has(F_DOOM_FREE | F_DOOM_INERT) {
            return false;
        }
        let u = self.unit_of(s);
        let mut notes = Notes::default();
        {
            let _g = self.wunit(u);
            if r.has(F_DOOM_FREE) {
                r.clear(F_DOOM_FREE | F_DOOM_INERT);
                r.status.store(super::S_CLEAN, Ordering::Release);
                self.finish_free(s, &mut notes);
            } else {
                r.clear(F_DOOM_INERT);
                self.finish_inert(s, &mut notes);
            }
            self.settle_structure(u, &mut notes);
        }
        self.dispatch(&mut notes, NIL);
        true
    }

    /// A validated node goes back to clean.
    /// A validated node goes back to clean, its reads subscribed afresh
    /// (the answers held, but what they consulted may have moved).
    /// `false`: a read broke meanwhile, run it.
    fn settle_clean(&self, s: Slot) -> bool {
        let r = self.rec(s);
        let u = self.unit_of(s);
        if r.has(F_DOOM_FREE | F_DOOM_INERT) {
            self.doomed(s);
            return true;
        }
        r.clear(F_STALE);
        let mut log = r
            .data()
            .res
            .as_mut()
            .map_or_else(Default::default, |c| std::mem::take(&mut c.log));
        let restore = |log: Vec<Pred>| {
            if let Some(c) = r.data().res.as_mut() {
                c.log = log;
            }
        };
        let ok = self.sread(u, || {
            log.iter_mut().all(|p| {
                let ok = matches!(self.recheck(s, p, false), Ok(Ok(())));
                if ok {
                    self.refresh_sites(s, p);
                }
                ok
            })
        });
        if !ok {
            restore(log);
            return false;
        }
        let lgen = r.lgen.load(Ordering::Acquire).wrapping_add(1) & 0x7fff_ffff;
        r.lgen.store(lgen, Ordering::Release);
        self.subscribe(s, u, lgen, &log);
        let ok = self.sread(u, || {
            log.iter().all(|p| matches!(self.recheck(s, p, false), Ok(Ok(()))))
        });
        restore(log);
        if !ok {
            return false;
        }
        if !self.leave_run(s, super::S_CLEAN) {
            return true;
        }
        let waiters = {
            self.leave_work_lists(s);
            std::mem::take(&mut r.data().waiters)
        };
        for (n, q) in waiters {
            self.wake(n, q);
        }
        self.guesses_settle(s);
        if r.has(F_STALE) {
            r.clear(F_STALE);
            self.touch(s, u32::MAX, &crate::api::Cause::Structure, false);
        }
        true
    }

    /// The definition entries a held read consults now.
    pub(crate) fn refresh_sites(&self, s: Slot, p: &mut Pred) {
        if let Pred::Def {
            f,
            at,
            mode,
            sites,
            ans,
            ..
        } = p
        {
            if matches!(ans, super::DAns::Loop(_)) {
                return;
            }
            let q = match mode {
                super::DMode::Use => self.q_def(*at, *f, false, s),
                m => self.q_seq(*at, *f, *m, false, s),
            };
            if let super::query::Q::Ans(o) = q {
                *sites = o.sites;
            }
        }
    }

    fn run_node(&self, s: Slot) {
        let r = self.rec(s);
        let ep = self.epoch.load(Ordering::Relaxed);
        // A run this evaluation counts in its report however the node came
        // to run (an edit, a wake, a node new since the last report).
        self.snapshot(s);
        let runs = {
            let mut d = r.data();
            if d.run_epoch != ep {
                d.run_epoch = ep;
                d.runs = 0;
                d.causes.retain(|_| false);
            }
            d.runs += 1;
            if d.res.is_some() {
                lock(&self.reruns).push((s, None));
            }
            d.runs
        };
        let limits = rlock(&self.limits).clone();
        if runs > u64::from(limits.runs_per_node) {
            let causes = r.data().causes.clone();
            self.fail(EvalError::RunawayNode {
                node: self.node_id(s),
                runs: u32::try_from(runs).unwrap_or(u32::MAX),
                causes,
            });
            self.park(
                s,
                Wait {
                    on: Subj::Child(0),
                    field: None,
                    prefix: None,
                    kind: "limit",
                    has_prev: false,
                },
                None,
                0,
            );
            return;
        }
        let total = self.c.runs.fetch_add(1, Ordering::Relaxed) + 1;
        if limits.total_runs.is_some_and(|t| total > t) || self.past_deadline() {
            self.c.runs.fetch_sub(1, Ordering::Relaxed);
            self.abort.store(true, Ordering::Relaxed);
            let mut e = lock(&self.error);
            if e.is_none() {
                *e = Some(EvalError::Deadline {
                    runs: total - 1,
                    waiters: Vec::new(),
                    report: Box::default(),
                });
            }
            drop(e);
            r.status.store(S_DIRTY, Ordering::Release);
            self.enqueue(s);
            return;
        }
        let (op, kept) = {
            let mut d = r.data();
            (d.op, d.kept.take())
        };
        let (prior, token) = match kept {
            Some(k) if k.token.is_some() => (k.log, k.token),
            _ => (Vec::new(), None),
        };
        let name = self.op_name(op);
        *lock(&self.t).runs_by_op.entry(name).or_default() += 1;
        r.clear(F_STALE);
        if r.has(super::F_NARROWED) {
            // Running again: the text's set until it narrows again.
            let may = self.analyze(s).may;
            self.set_may(s, may);
            r.clear(super::F_NARROWED);
        }
        // Under speculation a node with a result runs on its previous
        // inputs while other work is in flight: a guess, refuted if an
        // input changes.
        if self.spec.load(Ordering::Relaxed) >= 1
            && r.data().res.is_some()
            && (self.qlen.load(Ordering::Acquire) > 0
                || self.busy.load(Ordering::Acquire) > 1
                || !lock(&self.step_done).is_empty())
        {
            r.set(super::F_PREDRUN);
        } else {
            r.clear(super::F_PREDRUN);
        }
        let o = self.ops[op.0 as usize].clone();
        let mut cx = Ctx::new(self, s, token);
        // A resumed run keeps the reads its earlier attempts made.
        cx.log = prior;
        crate::obs::count!(Job);
        let res = {
            crate::obs::span!(Job);
            Self::set_in_run(true);
            let res = o.run(&mut cx);
            Self::set_in_run(false);
            res
        };
        cx.flush_side();
        if let Some(m) = cx.narrowed.take() {
            self.set_may(s, m.into());
            r.set(super::F_NARROWED);
            // Its waiters look again (those outside the set go on).
            let w = std::mem::take(&mut r.data().waiters);
            for (n, q) in w {
                self.wake(n, q);
            }
        }
        if let Ok(out) = res {
            if !self.commit(&mut cx, out) {
                r.data().kept = None;
                if let Some(w) = cx.wait.take() {
                    // A read waits on a node in work: it runs again once
                    // that node is done.
                    r.set(F_NEEDS_RUN);
                    self.park(s, w, None, 0);
                } else {
                    // A read broke before the commit: run again.
                    self.run_again(s);
                }
            }
        } else {
            let w = cx.wait.take().unwrap_or(Wait {
                on: Subj::Child(0),
                field: None,
                prefix: None,
                kind: "stop",
                has_prev: false,
            });
            if let Subj::Child(id) = w.on
                && id != 0
            {
                self.commit_structure(&mut cx);
                if let Some(c) = self.slot_of(id) {
                    let w = Wait { on: Subj::Node(c), ..w };
                    let kept = Kept {
                        log: std::mem::take(&mut cx.log),
                        token: cx.token.take(),
                    };
                    self.park(s, w, Some(kept), 0);
                    return;
                }
            }
            let kept = Kept {
                log: std::mem::take(&mut cx.log),
                token: cx.token.take(),
            };
            r.data().ex().ask = cx.ask;
            self.park(s, w, Some(kept), 0);
        }
    }

    fn run_again(&self, s: Slot) {
        let r = self.rec(s);
        if self.doomed(s) {
            return;
        }
        r.set(F_NEEDS_RUN);
        r.status.store(S_DIRTY, Ordering::Release);
        self.enqueue(s);
    }

    pub(crate) fn past_deadline(&self) -> bool {
        lock(&self.deadline).is_some_and(|d| std::time::Instant::now() >= d)
    }

    pub(crate) fn fail(&self, e: EvalError) {
        let mut g = lock(&self.error);
        if g.is_none() {
            *g = Some(e);
        }
        self.abort.store(true, Ordering::Relaxed);
    }

    /// Woken, it validates again the first time (what it waited for
    /// may leave its reads holding); after that it runs, so that a chain
    /// of waits is not validated over and over.
    fn park_validation(&self, s: Slot, w: Wait) {
        // Woken, it checks again; past one wait it runs instead, except
        // for a range (whose fold is cheap to compare, and whose window
        // may hold several nodes in work in turn).
        let again = w.kind == "range" || self.rec(s).data().x.as_ref().is_none_or(|e| e.wait_on.is_none());
        self.park_with(s, w, None, 1, !again);
    }

    /// Parks `s` on what `w` waits for.
    #[allow(clippy::needless_pass_by_value)]
    pub(crate) fn park(&self, s: Slot, w: Wait, kept: Option<Kept>, gated: u32) {
        self.park_with(s, w, kept, gated, true);
    }

    /// `park`; `needs_run` false: woken, it validates its committed log
    /// again rather than running.
    #[allow(clippy::needless_pass_by_value)]
    fn park_with(&self, s: Slot, w: Wait, kept: Option<Kept>, gated: u32, needs_run: bool) {
        let r = self.rec(s);
        if self.doomed(s) {
            return;
        }
        if needs_run {
            r.set(F_NEEDS_RUN);
        } else {
            r.clear(F_NEEDS_RUN);
        }
        let seq = r.park_seq.fetch_add(1, Ordering::AcqRel).wrapping_add(1) & 0x7fff_ffff;
        r.park_seq.store(seq, Ordering::Release);
        {
            let mut d = r.data();
            d.kept = kept;
            d.ex().wait_on = Some(w.clone());
            // (Only the waits a committed result keeps: see `commit`.)
            if let Subj::Node(x) = w.on
                && w.kind == "pending set"
            {
                d.ex().waited.push(WaitRec {
                    kind: w.kind,
                    by: x,
                    id: self.rec(x).id.load(Ordering::Acquire),
                    field: w.field,
                    prefix: w.prefix.clone(),
                });
            }
        }
        if gated == 1 {
            crate::obs::count!(Gated);
            self.c.gated.fetch_add(1, Ordering::Relaxed);
        } else {
            crate::obs::count!(Park);
            self.c.stops.fetch_add(1, Ordering::Relaxed);
        }
        if w.kind == "pending set" {
            self.c.pending_waits.fetch_add(1, Ordering::Relaxed);
        }
        {
            let name = self.op_name(r.data().op);
            let mut t = lock(&self.t);
            *t.waits_by_op.entry(name).or_default() += 1;
            *t.waits_by_kind.entry(w.kind).or_default() += 1;
            if let Subj::Node(x) = w.on {
                *t.waited.entry(x).or_default() += 1;
            }
        }
        if !self.leave_run(s, S_PARK) {
            return;
        }
        lock(&self.parked).push(s);
        match w.on {
            Subj::Node(x) => {
                let xr = self.rec(x);
                xr.data().waiters.push((s, seq));
                if !in_work(xr.st()) || xr.st() == S_FREE {
                    self.wake(s, seq);
                }
            }
            Subj::Env(f) => {
                lock(&self.env_waits).entry(f).or_default().push((s, seq));
                if self.q_env(f).is_some() {
                    self.wake(s, seq);
                }
            }
            Subj::Child(_) => {}
        }
    }

    /// The structure part of a stopped run (children it made before it
    /// stopped on one of them), so that they can run.
    fn commit_structure(&self, cx: &mut Ctx<'_>) {
        let s = cx.me;
        let u = cx.unit;
        let mut notes = Notes::default();
        {
            let _g = self.wunit(u);
            if self.rec(s).st() != S_RUN {
                return;
            }
            if !cx.env.is_empty() && self.rec(s).has(super::F_APP) {
                let au = self.rec(s).data().app_unit();
                let mut e = wlock(&self.unit(au).env);
                for &(f, x) in &cx.env {
                    e.insert(f, x);
                }
            }
            // Keep the committed kids not made again only if they come
            // before what this run made (a prefix): simplest, the new list.
            let kids = cx.kids.clone();
            let conts = cx.conts.clone();
            self.set_unfolding(s, &kids, &conts, &mut notes);
            self.settle_structure(u, &mut notes);
        }
        self.dispatch(&mut notes, s);
        let _ = Cause::Structure;
    }
}
