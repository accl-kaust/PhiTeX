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
    Shared, Slot, WaitRec, in_work, lock,
};
use crate::api::{Cause, EvalError};

thread_local! {
    /// This worker's queue shard index.
    static WORKER: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// In a job (free running): whether one is, the first node it made
    /// ready, how many it made ready.
    static NEXT: std::cell::Cell<(bool, Slot, u32)> = const { std::cell::Cell::new((false, NIL, 0)) };
}

/// What validating a log found.
pub(crate) enum Check {
    Holds,
    Broke(&'static str),
    Wait(Wait),
}

impl Shared {
    /// Queues a ready node while running free: on this worker's shard
    /// (what a worker makes ready, it likely runs next).
    #[allow(clippy::modulo_one)] // `QSHARDS` may be 1
    /// Queues a ready node; `false`: kept for this worker to run next.
    pub(crate) fn push_ready(&self, s: Slot) -> bool {
        // Successor affinity: the one node a job makes ready runs next on
        // the same worker (a chain of small nodes stays on a core that is
        // up to speed); a second one sends both to the shared queue.
        let kept = NEXT.with(|c| {
            let (on, local, n) = c.get();
            if !on {
                return None;
            }
            c.set((on, if n == 0 { s } else { NIL }, n + 1));
            Some((n, local))
        });
        if !self.affinity.load(Ordering::Relaxed) {
            // (Counted only: every successor is queued.)
            self.push_counted(s);
            return true;
        }
        match kept {
            Some((0, _)) => return false,
            Some((1, prev)) => self.push_counted(prev),
            _ => {}
        }
        self.push_counted(s);
        true
    }

    #[allow(clippy::modulo_one)] // `QSHARDS` may be 1
    fn push_counted(&self, s: Slot) {
        let i = WORKER.with(std::cell::Cell::get);
        self.qlen.fetch_add(1, Ordering::SeqCst);
        self.push_to(i % super::QSHARDS, s);
    }

    /// Moves an already counted ready node to a shard by its slot.
    #[allow(clippy::modulo_one)] // `QSHARDS` may be 1
    pub(crate) fn push_shard(&self, s: Slot) {
        self.push_to(s as usize % super::QSHARDS, s);
    }

    fn push_to(&self, i: usize, s: Slot) {
        let q = &self.shards[i];
        let l = self.rec(s).lab();
        let mut h = lock(&q.h);
        h.push(std::cmp::Reverse((l, s)));
        if l < q.top.load(Ordering::Relaxed) {
            q.top.store(l, Ordering::Release);
        }
    }

    /// Pops shard `q`'s first node (its lock held), keeping its top label.
    fn pop_from(q: &super::QShard, h: &mut super::ReadyHeap) -> Option<Slot> {
        let std::cmp::Reverse((_, s)) = h.pop()?;
        q.top.store(h.peek().map_or(u64::MAX, |r| r.0.0), Ordering::Release);
        Some(s)
    }

    #[allow(clippy::modulo_one)] // `QSHARDS` may be 1
    fn pop(&self) -> Option<Slot> {
        // Idle workers poll: an empty queue is seen without its lock.
        if self.qlen.load(Ordering::SeqCst) == 0 {
            return None;
        }
        if !self.stepped() {
            // The frontier: the shard whose first node comes first in the
            // text (a busy shard is passed for the next best; the last
            // resort waits for one).
            let mut order: [(u64, usize); super::QSHARDS] = [(u64::MAX, 0); super::QSHARDS];
            for (i, q) in self.shards.iter().enumerate() {
                order[i] = (q.top.load(Ordering::Acquire), i);
            }
            order.sort_unstable();
            for (k, &(top, i)) in order.iter().enumerate() {
                if top == u64::MAX {
                    break;
                }
                let q = &self.shards[i];
                let mut h = match q.h.try_lock() {
                    Ok(h) => h,
                    Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
                    Err(std::sync::TryLockError::WouldBlock) => {
                        if k + 1 < order.len() && order[k + 1].0 != u64::MAX {
                            continue;
                        }
                        lock(&q.h)
                    }
                };
                if let Some(s) = Self::pop_from(q, &mut h) {
                    self.busy.fetch_add(1, Ordering::SeqCst);
                    self.qlen.fetch_sub(1, Ordering::SeqCst);
                    return Some(s);
                }
            }
            return None;
        }
        let mut q = lock(&self.q);
        let std::cmp::Reverse((_, s)) = q.cur.pop()?;
        self.busy.fetch_add(1, Ordering::SeqCst);
        self.qlen.fetch_sub(1, Ordering::SeqCst);
        Some(s)
    }

    /// Whether the evaluation is quiescent: nothing queued, nobody busy.
    fn quiet(&self) -> bool {
        if !self.stepped() {
            // Queued or in process: a processed node's count goes after
            // what it queued.
            return self.work.load(Ordering::SeqCst) == 0;
        }
        // In steps the next step's nodes count in `qlen`: the step is
        // over when its own queue is empty and nobody is busy.
        if self.busy.load(Ordering::SeqCst) != 0 {
            return false;
        }
        let q = lock(&self.q);
        q.cur.is_empty() && self.busy.load(Ordering::SeqCst) == 0
    }

    /// One worker: processes nodes until the evaluation is quiescent.
    pub(crate) fn work(&self) {
        WORKER.with(|c| c.set(self.next_worker.fetch_add(1, Ordering::Relaxed)));
        #[cfg(feature = "profile")]
        let _flush = FlushOnExit;
        let mut idle = 0u32;
        // When this worker found nothing ready (its idle time, counted).
        let mut idle_since: Option<std::time::Instant> = None;
        let idle_add = |t: &mut Option<std::time::Instant>| {
            if let Some(t0) = t.take() {
                self.c
                    .idle_ns
                    .fetch_add(u64::try_from(t0.elapsed().as_nanos()).unwrap_or(0), Ordering::Relaxed);
            }
        };
        #[cfg(feature = "profile")]
        let mut idle_t: Option<std::time::Instant> = None;
        #[cfg(feature = "profile")]
        let idle_end = |t: &mut Option<std::time::Instant>| {
            if let Some(t0) = t.take() {
                #[allow(clippy::cast_possible_truncation)]
                crate::obs::add_impl("worker_idle", t0.elapsed().as_nanos() as u64);
            }
        };
        loop {
            if self.abort.load(Ordering::Relaxed) {
                #[cfg(feature = "profile")]
                idle_end(&mut idle_t);
                return;
            }
            if let Some(mut s) = self.pop() {
                idle_add(&mut idle_since);
                #[cfg(feature = "profile")]
                idle_end(&mut idle_t);
                idle = 0;
                loop {
                    NEXT.with(|c| c.set((true, NIL, 0)));
                    {
                        crate::obs::span!(RunBatch);
                        self.process(s);
                    }
                    self.merge();
                    let (_, local, n) = NEXT.with(|c| c.replace((false, NIL, 0)));
                    match n {
                        0 => {}
                        1 => {
                            self.c.sole_successor.fetch_add(1, Ordering::Relaxed);
                        }
                        _ => {
                            self.c.many_successors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    let kept = n == 1 && self.affinity.load(Ordering::Relaxed);
                    if kept && !self.abort.load(Ordering::Relaxed) {
                        self.c.kept_successors.fetch_add(1, Ordering::Relaxed);
                        // Its one successor, still counted busy and queued.
                        self.work.fetch_sub(1, Ordering::SeqCst);
                        s = local;
                        continue;
                    }
                    if kept {
                        self.push_counted(local);
                    }
                    break;
                }
                self.work.fetch_sub(1, Ordering::SeqCst);
                if self.busy.fetch_sub(1, Ordering::SeqCst) == 1 && self.qlen.load(Ordering::SeqCst) == 0 {
                    // Perhaps quiescent: the sleepers look.
                    self.wake_idle(true);
                }
                continue;
            }
            // Landed items whose list was busy when their lander looked.
            self.merge();
            if self.quiet() {
                idle_add(&mut idle_since);
                #[cfg(feature = "profile")]
                idle_end(&mut idle_t);
                return;
            }
            #[cfg(feature = "profile")]
            if idle_t.is_none() {
                idle_t = Some(std::time::Instant::now());
                crate::obs::add_impl("idle_busy_others", self.busy.load(Ordering::Relaxed) as u64);
                crate::obs::add_impl("idle_parked", lock(&self.parked).len() as u64);
            }
            if idle_since.is_none() {
                idle_since = Some(std::time::Instant::now());
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
            } else if idle < 128 {
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
        self.check_log_from(me, log, 0).0
    }

    /// `check_log_in` from read `from` on (the reads before it held when
    /// last checked, and nothing they read has changed since: a change
    /// would have touched the node). Also returns where it stopped.
    pub(crate) fn check_log_from(&self, me: Slot, log: &[Pred], from: usize) -> (Check, usize) {
        // Following-sibling reads in order share one walk.
        let mut cur: Option<(usize, Slot)> = None;
        for (k, p) in log.iter().enumerate().skip(from) {
            if let Pred::Sib { i, ans } = p {
                let i = *i as usize;
                let (got, _) = match cur {
                    Some((j, x)) if j < i => self.q_sib_from(me, x, j + 1, i),
                    _ => self.q_sib(me, i),
                };
                cur = got.map(|g| (i, g.0));
                match (got, ans) {
                    (a, b) if a == *b => continue,
                    (Some(a), Some(b)) if a.0 == b.0 && a.1 == b.1 => return (Check::Broke("following: changed"), k),
                    _ => return (Check::Broke("following: gone"), k),
                }
            }
            match self.recheck(me, p, true) {
                Err(w) => return (Check::Wait(w), k),
                Ok(Err(why)) => return (Check::Broke(why), k),
                Ok(Ok(())) => {}
            }
        }
        (Check::Holds, log.len())
    }

    /// An in-work pending claimer before `s` whose claim may cover it.
    pub(crate) fn held_by(&self, s: Slot) -> Option<Slot> {
        let u = self.unit_of(s);
        // No pending claimer: nothing to read (and no writer to wait out).
        if self.unit(u).pclaim.read().is_empty() {
            return None;
        }
        self.sread(u, || self.held_by_in(s))
    }

    #[inline(never)]
    pub(crate) fn held_by_in(&self, s: Slot) -> Option<Slot> {
        let u = self.unit_of(s);
        let list = self.unit(u).pclaim.read();
        if list.is_empty() {
            return None;
        }
        let ls = self.rec(s).lab();
        // The list is sorted by label: only the claimers before `s`.
        let end = list.partition_point(|&c| self.rec(c).lab() < ls);
        let lo = self.unit(u).pclaim_lo.load(Ordering::Acquire).min(end);
        for &c in list[lo..end].iter().rev() {
            if c == s || !in_work(self.rec(c).st()) || !self.rec(c).has(super::F_IN_PCLAIM) {
                continue;
            }
            let limit = {
                let d = self.rec(c).data();
                d.x.as_ref().and_then(|e| e.early_claim).or_else(|| d.stat.limit())
            };
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
        #[cfg(feature = "observe")]
        {
            *super::lock(&self.selects).entry(s).or_default() += 1;
            crate::trace::node(self, s, "select", "");
        }
        let r = self.rec(s);
        let mode = self.spec.load(Ordering::Relaxed);
        // A job the scheduler chose speculatively always may guess; any
        // other run guesses on a definer in work as the mode allows.
        let chosen = r.flags.fetch_and(!super::F_SPECJOB, Ordering::AcqRel) & super::F_SPECJOB != 0;
        let spec = chosen || mode == 2 || (mode == 1 && r.data().res.is_some());
        self.set_spec_run(spec && mode != 0);
        // The version its reads start at (a check of its kept or committed
        // log included).
        let ver = self.version();
        self.process_job(s, ver);
        self.set_spec_run(false);
    }

    fn process_job(&self, s: Slot, ver: u64) {
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
        if r.has(F_DOOM_FREE | F_DOOM_INERT) {
            self.land(s, move |sh| {
                sh.doomed(s);
            });
            return;
        }
        if let Some(c) = self.held_by(s) {
            if self.rec(c).st() == super::S_STEP {
                self.land(s, move |sh| sh.defer_step(s, "claim"));
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
            let kept = r.data().kept.take().map(|k| *k);
            self.land(s, move |sh| sh.park_with(s, w, kept, 1, run));
            return;
        }
        let has_res = r.data().res.is_some();
        let kept_log = r.data().kept.as_ref().map(|k| k.log.clone());
        if let Some(log) = kept_log {
            match self.check_log(s, &log) {
                Check::Wait(w) => {
                    let token = r.data().kept.take().and_then(|k| k.token);
                    self.land(s, move |sh| sh.park(s, w, Some(Kept { log, token }), 0));
                    return;
                }
                Check::Broke(why) => {
                    r.data().kept = None;
                    self.c.restarted.fetch_add(1, Ordering::Relaxed);
                    *self.tal().restarted_by.entry(why).or_default() += 1;
                }
                Check::Holds => {
                    // Still waiting where it stopped: park again unrun.
                    let ask = r.data().x.as_ref().and_then(|e| e.ask);
                    if let Some(w) = ask.and_then(|a| self.reask(s, a)) {
                        let kept = r.data().kept.take().map(|k| *k);
                        self.land(s, move |sh| sh.park(s, w, kept, 1));
                        return;
                    }
                    self.c.resumed.fetch_add(1, Ordering::Relaxed);
                }
            }
        } else if st == S_DIRTY && has_res && !r.has(F_NEEDS_RUN) {
            let log = r.data().res.as_ref().map_or_else(Default::default, |c| c.log.clone());
            r.set(super::F_CHECKING);
            let from = r.data().x.as_ref().map_or(0, |e| e.check_from as usize).min(log.len());
            let u = self.unit_of(s);
            let tk = std::time::Instant::now();
            let (ck, at) = self.sread(u, || self.check_log_from(s, &log, from));
            self.c
                .check_ns
                .fetch_add(u64::try_from(tk.elapsed().as_nanos()).unwrap_or(0), Ordering::Relaxed);
            if let Some(e) = r.data().x.as_mut() {
                e.check_from = 0;
            }
            if matches!(ck, Check::Wait(_)) {
                #[allow(clippy::cast_possible_truncation)]
                {
                    let mut d = r.data();
                    let e = d.ex();
                    e.check_from = at as u32;
                    e.check_stuck = at == from;
                }
            }
            match ck {
                Check::Holds => {
                    let done = self.land_clean(s, ver);
                    r.clear(super::F_CHECKING);
                    if done {
                        return;
                    }
                }
                Check::Wait(w) => {
                    // Validation parks with nothing kept: it re-validates.
                    r.clear(super::F_CHECKING);
                    self.land(s, move |sh| sh.park_validation(s, w));
                    return;
                }
                Check::Broke(_) => {
                    r.clear(super::F_CHECKING);
                    // The rerun's cause is the read that broke (not the
                    // last touch, which may have held).
                    if let Some(f) = self.first_broken_field(s, &log) {
                        let last = super::lock(&self.reruns).iter().rev().find(|e| e.0 == s).map(|e| e.1);
                        if let Some(Some(crate::api::Rerun::Changed { field, .. })) = last
                            && field != f
                        {
                            let old = Self::read_value(&r.data(), f).unwrap_or(crate::values::ValueId::UNIT);
                            self.note_rerun(
                                s,
                                crate::api::Rerun::Changed {
                                    field: f,
                                    old,
                                    new: crate::values::ValueId::UNIT,
                                },
                            );
                        }
                    }
                    if r.has(super::F_PREDREAD | super::F_PROJREAD) {
                        let f = self.broken_field(s, &log);
                        self.prediction_broke(s, f);
                    }
                }
            }
        }
        if let Some(w) = self.gate(s).or_else(|| self.hold_refuted(s)) {
            if let Subj::Node(x) = w.on
                && self.rec(x).st() == super::S_STEP
            {
                self.land(s, move |sh| sh.defer_step(s, "gated"));
                return;
            }
            let kept = r.data().kept.take().map(|k| *k);
            self.land(s, move |sh| sh.park(s, w, kept, 1));
            return;
        }
        self.run_node(s, ver);
    }

    /// The field of the first definition read in `log` that no longer
    /// holds.
    fn first_broken_field(&self, s: Slot, log: &[super::Pred]) -> Option<crate::values::Field> {
        log.iter().find_map(|p| match p {
            super::Pred::Def { f, .. } if matches!(self.check_log(s, std::slice::from_ref(p)), Check::Broke(_)) => {
                Some(*f)
            }
            _ => None,
        })
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

    /// A node whose last run read a field no longer guessed (refuted in
    /// this evaluation) from a definer now in work waits for that definer
    /// instead of running ahead on the old value.
    fn hold_refuted(&self, s: Slot) -> Option<Wait> {
        if !self.any_no_guess.load(Ordering::Acquire) {
            return None;
        }
        let d = self.rec(s).data();
        let log = &d.res.as_ref()?.log;
        log.iter().find_map(|p| match p {
            super::Pred::Def {
                f,
                ans: super::DAns::Site { node, .. },
                ..
            } if *node != s && self.busy(*node, s) && !self.guess_ok(*f) => Some(super::query::wait_node(
                *node,
                Some(*f),
                "gated",
                self.rec(*node).has(super::F_DONE),
            )),
            _ => None,
        })
    }

    /// A node whose read hint names a field still in work before it waits
    /// without running.
    pub(crate) fn gate(&self, s: Slot) -> Option<Wait> {
        let u = self.unit_of(s);
        if self.rec(s).data().stat.reads().is_none_or(Vec::is_empty) {
            return None;
        }
        self.sread(u, || self.gate_in(s))
    }

    /// Narrows `s`'s may-define set; its waiters look again (those
    /// outside the set go on).
    /// Wakes the nodes parked on `s` (they look again).
    pub(crate) fn wake_all_waiters(&self, s: Slot) {
        let w = std::mem::take(&mut self.rec(s).data().waiters);
        for (n, q) in w {
            self.wake(n, q);
        }
    }

    pub(crate) fn narrow_now(&self, s: Slot, m: crate::api::MayDefine) {
        let r = self.rec(s);
        self.set_may(s, m.into());
        r.set(super::F_NARROWED);
        let w = std::mem::take(&mut r.data().waiters);
        for (n, q) in w {
            self.wake(n, q);
        }
    }

    fn gate_in(&self, s: Slot) -> Option<Wait> {
        // The hint is the text's: analysed once, not per attempt.
        let fields = self.rec(s).data().stat.reads().cloned()?;
        for f in fields {
            if self.loop_vals.read().contains_key(&f) {
                continue;
            }
            // An accumulator read: held while a contributor before it is in
            // work (from the unit's start: a later anchor waits on more).
            if self.accs.contains_key(&f) {
                if let super::query::Q::Wait(mut w) = self.q_range(s, f, None, None, true, s) {
                    w.kind = "gated";
                    return Some(w);
                }
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
        if !self.leave_run(s, S_DIRTY) {
            return;
        }
        crate::obs::count!(Gated);
        self.c.gated.fetch_add(1, Ordering::Relaxed);
        *self.tal().waits_by_kind.entry(kind).or_default() += 1;
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
                if self.doomed(s) {
                    return false;
                }
                // The mark was taken back: on to `to` after all.
                r.status.store(to, Ordering::Release);
                return true;
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
            // A release may have taken the mark back before the lock.
            if !r.has(F_DOOM_FREE | F_DOOM_INERT) {
                return false;
            }
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

    /// A validated node goes back to clean, its reads subscribed afresh
    /// (the answers held, but what they consulted may have moved): the
    /// worker's part, landed for the merger. `false`: a read broke
    /// meanwhile, run it.
    fn land_clean(&self, s: Slot, ver: u64) -> bool {
        let r = self.rec(s);
        let u = self.unit_of(s);
        r.clear(F_STALE);
        // (A copy: the merger may read the committed log meanwhile.)
        let mut log = r.data().res.as_ref().map_or_else(Default::default, |c| c.log.clone());
        if !self.recheck_chunked(s, u, &mut log, true).0 {
            return false;
        }
        let lgen = r.lgen.load(Ordering::Acquire).wrapping_add(1) & 0x7fff_ffff;
        r.lgen.store(lgen, Ordering::Release);
        self.subscribe(s, u, lgen, &log);
        let keys = self.keys_of(&log);
        self.land(s, move |sh| sh.merge_clean(s, ver, keys, log));
        true
    }

    /// The merger's part of a check that held: verified against what
    /// changed since its version, then clean.
    fn merge_clean(&self, s: Slot, ver: u64, keys: Vec<u64>, mut log: Vec<Pred>) {
        let r = self.rec(s);
        let u = self.unit_of(s);
        // (As in `merge_commit`: a mark taken back leaves it to go on.)
        if (r.has(F_DOOM_FREE | F_DOOM_INERT) && self.doomed(s)) || r.st() != S_RUN {
            return;
        }
        let stale = r.flags.fetch_and(!F_STALE, Ordering::AcqRel) & F_STALE != 0;
        if stale || self.moved_since(ver, &keys) {
            if !self.recheck_chunked(s, u, &mut log, true).0 {
                self.run_again(s);
                return;
            }
            let lgen = r.lgen.load(Ordering::Acquire).wrapping_add(1) & 0x7fff_ffff;
            r.lgen.store(lgen, Ordering::Release);
            self.subscribe(s, u, lgen, &log);
            if !self.recheck_chunked(s, u, &mut log, false).0 {
                self.run_again(s);
                return;
            }
        }
        if let Some(c) = r.data().res.as_mut() {
            c.log = log;
            c.read_ix = None;
        }
        if !self.leave_run(s, super::S_CLEAN) {
            return;
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
            if *mode == super::DMode::Use {
                // Held with an equal value from another site: follow the
                // site, so later changes there reach this reader.
                if let super::query::Q::Ans(o) = self.q_def(*at, *f, false, s)
                    && o.ans != *ans
                    && super::query::same_value(&o.ans, ans)
                {
                    *ans = o.ans;
                }
                return;
            }
            if let super::query::Q::Ans(o) = self.q_seq(*at, *f, *mode, false, s) {
                *sites = o.sites.into_boxed_slice();
            }
        }
    }

    fn run_node(&self, s: Slot, ver: u64) {
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
            // (Completed runs count, counted at commit: an attempt that
            // stops is not one.)
            if d.res.is_some() {
                lock(&self.reruns).push((s, None));
            }
            d.runs + 1
        };
        let limits = &self.limits;
        if runs > limits.runs_per_node {
            let causes = r.data().causes.clone();
            self.fail(EvalError::RunawayNode {
                node: self.node_id(s),
                runs,
                causes,
            });
            let w = Wait {
                on: Subj::Child(0),
                field: None,
                prefix: None,
                kind: "limit",
                has_prev: false,
            };
            self.land(s, move |sh| sh.park(s, w, None, 0));
            return;
        }
        self.c.runs.fetch_add(1, Ordering::Relaxed);
        let total = if limits.total_runs.is_some() {
            self.c.runs.load(Ordering::Relaxed)
        } else {
            0
        };
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
            self.land(s, move |sh| sh.requeue(s, false));
            return;
        }
        let (op, kept) = {
            let mut d = r.data();
            (d.op, d.kept.take().map(|k| *k))
        };
        let (prior, token) = match kept {
            Some(k) if k.token.is_some() => (k.log, k.token),
            _ => (Vec::new(), None),
        };
        let name = self.op_name(op);
        *self.tal().runs_by_op.entry(name).or_default() += 1;
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
                || self.landed.load(Ordering::Acquire) > 0)
        {
            r.set(super::F_PREDRUN);
        } else {
            r.clear(super::F_PREDRUN);
        }
        let o = self.ops[op.0 as usize].clone();
        if token.is_none()
            && let Some(e) = r.data().x.as_mut()
        {
            e.early_claim = None;
        }
        if token.is_none() && r.has(super::F_PCLAIM) && !r.has(super::F_IN_PCLAIM) {
            self.enter_work_lists(s);
        }
        let mut cx = Ctx::new(self, s, token);
        // A resumed run keeps the reads its earlier attempts made.
        cx.prior = prior.len();
        cx.log = prior;
        // Its in-order sibling walk goes on where the earlier attempts left it.
        if let Some((i, x, gn)) = cx.log.iter().rev().find_map(|p| match p {
            Pred::Sib {
                i,
                ans: Some((x, gn, _)),
            } => Some((*i as usize, *x, *gn)),
            _ => None,
        }) {
            cx.sib_cur.set(Some((i, x, gn)));
        }
        crate::obs::count!(Job);
        let res = {
            crate::obs::span!(Job);
            Self::set_in_run(true);
            #[cfg(not(target_arch = "wasm32"))]
            let t0 = std::time::Instant::now();
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| o.run(&mut cx)));
            #[cfg(not(target_arch = "wasm32"))]
            {
                cx.run_ns = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
                *self.tal().run_ns.entry(s).or_default() += cx.run_ns;
                self.c.op_ns.fetch_add(cx.run_ns, Ordering::Relaxed);
            }
            Self::set_in_run(false);
            match res {
                Ok(res) => res,
                Err(p) => {
                    // The evaluation fails, naming the node; the node runs
                    // again in the next one.
                    let message = p
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_string())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "(non-string panic)".into());
                    self.fail(EvalError::OpPanicked {
                        node: self.node_id(s),
                        op: name.to_string(),
                        message,
                    });
                    drop(cx);
                    self.land(s, move |sh| sh.requeue(s, true));
                    return;
                }
            }
        };
        cx.flush_side();
        // A speculative run's narrowing rests on answers in work: it holds
        // from its commit (its waiters stay held until then).
        let narrowed = cx.narrowed.take();
        let late = narrowed.is_some() && Self::spec_run();
        let mut at_commit = None;
        if let Some(m) = narrowed {
            if late {
                at_commit = Some(m);
            } else {
                // A declaration, not a fact: narrowed at once (readers
                // passing it are verified by its commit); its waiters are
                // woken by the merger.
                self.set_may(s, m.into());
                r.set(super::F_NARROWED);
                self.land(s, move |sh| sh.wake_all_waiters(s));
            }
        }
        if let Ok(out) = res {
            let tc = std::time::Instant::now();
            self.land_commit(&mut cx, out, ver, at_commit);
            self.c
                .commit_ns
                .fetch_add(u64::try_from(tc.elapsed().as_nanos()).unwrap_or(0), Ordering::Relaxed);
        } else {
            let pclaim = r.has(super::F_PCLAIM);
            let spec = Self::spec_run();
            // A pending claimer that set its claim before it stopped:
            // siblings past the claim go on now (their view of it is a
            // wait decision, not a fact: set at once; its waiters are
            // woken by the merger).
            let mut early = None;
            if let Some(n) = cx.claim.filter(|_| !spec && pclaim) {
                let old = r.data().ex().early_claim.replace(n);
                if n == 0 {
                    // Claims nothing while stopped: not a claimer later
                    // nodes look past (it joins again when it runs anew).
                    self.leave_pclaim(s);
                }
                early = (old != Some(n)).then_some(n);
            }
            let w = cx.wait.take().unwrap_or(Wait {
                on: Subj::Child(0),
                field: None,
                prefix: None,
                kind: "stop",
                has_prev: false,
            });
            let kept = Kept {
                log: std::mem::take(&mut cx.log),
                token: cx.token.take(),
            };
            let ask = cx.ask;
            // The children it made before it stopped on one of them: made
            // now, so that they can run.
            let partial = matches!(w.on, Subj::Child(id) if id != 0).then(|| cx.take_out(ver, Vec::new(), false));
            self.land(s, move |sh| sh.merge_stop(s, early, w, kept, ask, partial));
        }
    }

    /// The merger's part of a run that stopped.
    fn merge_stop(
        &self,
        s: Slot,
        early: Option<usize>,
        w: Wait,
        kept: Kept,
        ask: Option<super::Ask>,
        partial: Option<super::ctx::RunOut>,
    ) {
        let r = self.rec(s);
        // Its claim changed: the siblings held by it look again.
        if early.is_some() {
            let ws = std::mem::take(&mut r.data().waiters);
            for (x, q) in ws {
                self.wake(x, q);
            }
        }
        if let Some(mut o) = partial
            && let Subj::Child(id) = w.on
        {
            self.commit_structure(&mut o);
            if let Some(c) = self.slot_of(id) {
                let w = Wait { on: Subj::Node(c), ..w };
                self.park(s, w, Some(kept), 0);
                return;
            }
        }
        r.data().ex().ask = ask;
        self.park(s, w, Some(kept), 0);
    }

    /// Back to the queue (`run`: to run, not to check).
    pub(crate) fn requeue(&self, s: Slot, run: bool) {
        if self.doomed(s) {
            return;
        }
        let r = self.rec(s);
        if run {
            r.set(F_NEEDS_RUN);
        }
        r.status.store(S_DIRTY, Ordering::Release);
        self.enqueue(s);
    }

    pub(crate) fn run_again(&self, s: Slot) {
        let r = self.rec(s);
        if self.doomed(s) {
            return;
        }
        r.set(F_NEEDS_RUN);
        r.status.store(S_DIRTY, Ordering::Release);
        self.enqueue(s);
    }

    pub(crate) fn past_deadline(&self) -> bool {
        self.deadline.is_some_and(|d| std::time::Instant::now() >= d)
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
        // Woken, it checks again from where this check waited (the reads
        // before held, and a change to any of them touches it, which starts
        // the check over): a chain of waits costs one pass over the log.
        // A repeat wait at the same claim or scope effect makes no
        // progress: it runs. (A wait on a definition or a pending set is
        // settled by its definer, often the same way: it checks again.)
        let again = !matches!(w.kind, "claim" | "scope effect")
            || self
                .rec(s)
                .data()
                .x
                .as_ref()
                .is_none_or(|e| e.wait_on.is_none() || !e.check_stuck);
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
            d.kept = kept.map(Box::new);
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
        #[cfg(feature = "observe")]
        crate::trace::node(self, s, if gated == 1 { "gate" } else { "park" }, w.kind);
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
            let mut t = self.tal();
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
                // (A claim wait decided before this merge: the claimer may
                // have let it go since.)
                if !in_work(xr.st()) || xr.st() == S_FREE || (w.kind == "claim" && self.held_by(s) != Some(x)) {
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
    fn commit_structure(&self, cx: &mut super::ctx::RunOut) {
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
                let mut e = self.unit(au).env.write();
                for &(f, x) in &cx.env {
                    self.note(super::merge::ke(f));
                    e.insert(f, x);
                }
            }
            // Keep the committed kids not made again only if they come
            // before what this run made (a prefix): simplest, the new list.
            let kids = cx.kids.clone();
            let conts = cx.conts.clone();
            let anchors = cx.anchors.clone();
            self.set_unfolding(s, &kids, &conts, &anchors, &mut notes);
            self.settle_structure(u, &mut notes);
        }
        self.dispatch(&mut notes, s);
        let _ = Cause::Structure;
    }
}

/// Merges a worker's probe totals when it leaves (profiling builds).
#[cfg(feature = "profile")]
struct FlushOnExit;

#[cfg(feature = "profile")]
impl Drop for FlushOnExit {
    fn drop(&mut self) {
        crate::obs::flush();
    }
}
