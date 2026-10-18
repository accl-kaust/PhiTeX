//! The commit of a run, on the worker that ran it: re-validate the reads
//! in the committed view under the unit's write lock, write the results,
//! subscribe the reads, and wake the waiters.

use std::sync::atomic::Ordering;

use super::ctx::Ctx;
use super::order::Notes;
use super::{
    ClaimS, Committed, DAns, DMode, Def, DefVal, F_APP, F_DONE, F_FWD, F_HAS_OUT, F_NEEDS_RUN, F_SENS, NIL, Pred,
    S_CLEAN, S_FREE, S_RUN, Shared, Slot, Sub, in_work, lock,
};
use crate::api::{Cause, EvalError};
use crate::values::{Field, ValueId, hash_words};

fn dans_hash(a: &DAns) -> u64 {
    match a {
        DAns::Site { node, gn, val } => hash_words(1, &[u64::from(*node), u64::from(*gn), defval_hash(*val)]),
        DAns::Env(v) => hash_words(2, &[v.map_or(u64::MAX, ValueId::raw)]),
        DAns::Loop(v) => hash_words(3, &[v.raw()]),
        DAns::Seq(v) => hash_words(4, &[v.map_or(u64::MAX, ValueId::raw)]),
    }
}

fn defval_hash(v: DefVal) -> u64 {
    match v {
        DefVal::Pending => 1,
        DefVal::Val(x) => hash_words(2, &[x.raw()]),
        DefVal::Push(x) => hash_words(3, &[x.raw()]),
        DefVal::Take => 4,
        DefVal::Repl(x) => hash_words(5, &[x.raw()]),
    }
}

/// The value part of a read's answer (what a re-run would see).
fn pred_value_hash(sh: &Shared, p: &Pred) -> u64 {
    let o = |x: Option<ValueId>| x.map_or(u64::MAX, ValueId::raw);
    match p {
        Pred::Def { f, ans, mode, .. } => {
            let v = match ans {
                DAns::Site { val, .. } => defval_hash(*val),
                a => dans_hash(a),
            };
            hash_words(10, &[f.0.raw(), *mode as u64, v])
        }
        Pred::Out { val, .. } => hash_words(11, &[o(*val)]),
        Pred::Range { f, val, .. } => hash_words(12, &[f.0.raw(), val.raw()]),
        Pred::Env { f, val } => hash_words(13, &[f.0.raw(), o(*val)]),
        Pred::Kind { val, .. } => hash_words(14, &[o(*val)]),
        Pred::IsLoop { f, val } => hash_words(21, &[f.0.raw(), u64::from(*val)]),
        Pred::Sib { i, ans } => hash_words(15, &[u64::from(*i), ans.map_or(0, |a| a.2)]),
        // The run gets the node found, not the texts passed over.
        Pred::PrevSib { ans, .. } => hash_words(16, &[ans.map_or(u64::MAX, |a| sh.rec(a).id.load(Ordering::Acquire))]),
        Pred::Claimed { list } => hash_words(17, &list.iter().map(|x| x.2).collect::<Vec<_>>()),
        Pred::Stream { seen, outs, .. } => {
            let mut w: Vec<u64> = seen.iter().map(|x| x.2).collect();
            w.extend(outs.iter().map(|x| x.raw()));
            hash_words(18, &w)
        }
        Pred::App { val, .. } => hash_words(19, &[val.raw()]),
        Pred::At { node, .. } => hash_words(20, &[sh.rec(*node).id.load(Ordering::Acquire)]),
    }
}

impl Shared {
    /// Its realised depth: one more than the deepest run of this
    /// evaluation it read without a guess.
    /// Whether `s`'s committed run read `f` from `n` with a value equal to
    /// `val` (through projection `proj`).
    fn read_as_before(&self, s: Slot, n: Slot, f: Field, proj: u16, val: super::DefVal) -> bool {
        let d = self.rec(s).data();
        let Some(res) = d.res.as_ref() else { return false };
        res.log.iter().any(|p| match p {
            Pred::Def {
                f: g,
                ans: super::DAns::Site { node, val: old, .. },
                ..
            } if *g == f && *node == n => {
                *old == val
                    || (proj != 0
                        && matches!((old, val), (super::DefVal::Val(x), super::DefVal::Val(y))
                            if self.project(proj, *x) == self.project(proj, y)))
            }
            _ => false,
        })
    }

    fn stamp_depth(&self, s: Slot, log: &[Pred], run_ns: u64) {
        let ep = self.epoch.load(Ordering::Relaxed) & 0xffff_ffff;
        let dep = |n: Slot| {
            let x = self.rec(n).rdep.load(Ordering::Acquire);
            if x >> 32 == ep { x & 0xffff_ffff } else { 0 }
        };
        let time = |n: Slot| {
            if dep(n) > 0 {
                self.rec(n).rtime.load(Ordering::Acquire)
            } else {
                0
            }
        };
        let mut m = 0;
        let mut tm = 0;
        for p in log {
            let n = match p {
                // A prediction did not wait (a previous value's guess
                // finds its definer unstamped when it read).
                Pred::Def {
                    f,
                    spec,
                    proj,
                    ans: super::DAns::Site { node, val, .. },
                    ..
                } => {
                    // A guess (a value read from a definer in work, a
                    // prediction) that held did not wait.
                    // (A definer new in this evaluation had nothing to
                    // guess from: its reader waited for it.)
                    if spec.is_some()
                        && (self.rec(*node).data().born != self.epoch.load(Ordering::Relaxed)
                            || matches!(val, super::DefVal::Val(x) if self.predicted(*node, *f) == Some(*x)))
                    {
                        continue;
                    }
                    // Read as its previous run read it: it could have gone
                    // on with that value, as a guess.
                    if dep(*node) > m && self.read_as_before(s, *node, *f, *proj, *val) {
                        continue;
                    }
                    *node
                }
                Pred::Out { node, .. } | Pred::App { node, .. } => *node,
                Pred::Range { last, .. } if *last != super::NIL => *last,
                _ => continue,
            };
            if n != s {
                m = m.max(dep(n));
                tm = tm.max(time(n));
            }
        }
        let d = m + 1;
        let t = tm.saturating_add(run_ns);
        self.rec(s).rtime.store(t, Ordering::Release);
        self.rec(s).rdep.store((ep << 32) | d, Ordering::Release);
        self.c.realised.fetch_max(d, Ordering::Relaxed);
        self.c.realised_ns.fetch_max(t, Ordering::Relaxed);
    }

    /// The worker's part of a commit: its reads checked once more (a
    /// held read follows an equal value's new site), subscribed under a
    /// new log generation, and the result landed for the merger.
    pub(crate) fn land_commit(
        &self,
        cx: &mut Ctx<'_>,
        out: ValueId,
        ver: u64,
        narrowed: Option<crate::api::MayDefine>,
    ) {
        let s = cx.me;
        let u = cx.unit;
        let r = self.rec(s);
        let pread = Self::take_pred_read();
        // The reads of a resumed run's earlier attempts: checked once more,
        // following an equal value's new site (the reads this attempt made
        // are verified at the merge).
        let n = cx.prior.min(cx.log.len());
        let (ok, wait) = self.recheck_chunked(s, u, &mut cx.log[..n], true);
        if !ok {
            // A read waits on an element in work: the run waits for it.
            if pread && wait.is_none() {
                self.c.predictions_refuted.fetch_add(1, Ordering::Relaxed);
                r.set(super::F_PREDREF);
                self.go_conservative(s);
            }
            self.land(s, move |sh| sh.not_committed(s, wait));
            return;
        }
        let lgen = r.lgen.load(Ordering::Acquire).wrapping_add(1) & 0x7fff_ffff;
        r.lgen.store(lgen, Ordering::Release);
        self.subscribe(s, u, lgen, &cx.log);
        let keys = self.keys_of(&cx.log);
        let mut o = cx.take_out(ver, keys, pread);
        o.narrowed = narrowed;
        self.land(s, move |sh| sh.merge_commit(o, out));
    }

    /// Asks the reads of `log` again (`refresh`: a held read follows an
    /// equal value's new site), a chunk of reads per consistent view of
    /// the unit (a structural write retries a chunk, not the log). Whether
    /// they hold, and what the first that waits waits on.
    pub(crate) fn recheck_chunked(
        &self,
        s: Slot,
        u: u32,
        log: &mut [Pred],
        refresh: bool,
    ) -> (bool, Option<super::query::Wait>) {
        const CHUNK: usize = 32;
        for part in log.chunks_mut(CHUNK) {
            let mut wait = None;
            let ok = self.sread(u, || {
                super::query::sib_cursor_reset();
                part.iter_mut().all(|p| match self.recheck(s, p, false) {
                    Ok(Ok(())) => {
                        if refresh {
                            self.refresh_sites(s, p);
                        }
                        true
                    }
                    Err(w) => {
                        wait = Some(w);
                        false
                    }
                    Ok(Err(_)) => false,
                })
            });
            if !ok {
                return (false, wait);
            }
        }
        (true, None)
    }

    /// A run that did not commit: it waits on what a read found in work,
    /// or runs again.
    pub(crate) fn not_committed(&self, s: Slot, wait: Option<super::query::Wait>) {
        self.rec(s).data().kept = None;
        if let Some(w) = wait {
            self.rec(s).set(F_NEEDS_RUN);
            self.park(s, w, None, 0);
        } else {
            self.run_again(s);
        }
    }

    /// The merger's part of a commit: verified against what changed since
    /// the run's version, then written to the store.
    pub(crate) fn merge_commit(&self, mut o: super::ctx::RunOut, out: ValueId) {
        let narrowed = o.narrowed.take();
        let s = o.me;
        let r = self.rec(s);
        // Doomed: freed or made inert here. A mark another merge took back
        // meanwhile leaves it to commit after all.
        if (r.has(super::F_DOOM_FREE | super::F_DOOM_INERT) && self.doomed(s)) || r.st() != S_RUN {
            return;
        }
        let late = narrowed.is_some();
        if self.commit(&mut o, out) {
            if late && let Some(m) = narrowed {
                self.narrow_now(s, m);
            }
        } else {
            let w = o.wait.take();
            self.not_committed(s, w);
        }
    }

    /// Commits a landed run; `false`: a read broke, run again (`cx.wait`:
    /// it waits).
    pub(crate) fn commit(&self, cx: &mut super::ctx::RunOut, out: ValueId) -> bool {
        #[cfg(feature = "observe")]
        crate::trace::node(self, cx.me, "commit", "");
        let s = cx.me;
        let u = cx.unit;
        let r = self.rec(s);
        crate::obs::span!(Finish);
        let mut notes = Notes::default();
        let waiters;
        let mut restructured;
        let pread = cx.pread;
        // Verified at the merge it lands in: only when something it read
        // (or the structure it read through) changed since its version are
        // its reads asked again.
        let stale = r.flags.fetch_and(!super::F_STALE, Ordering::AcqRel) & super::F_STALE != 0;
        if stale || self.moved_since(cx.ver, &cx.keys) {
            let (ok, wait) = self.recheck_chunked(s, u, &mut cx.log, true);
            if !ok {
                if pread && wait.is_none() {
                    self.c.predictions_refuted.fetch_add(1, Ordering::Relaxed);
                    r.set(super::F_PREDREF);
                    self.go_conservative(s);
                }
                cx.wait = wait;
                return false;
            }
            // The sites it consults may have moved: subscribed afresh, then
            // checked once more (a write between the check and the new
            // subscription notified the old one, which no longer counts).
            let lgen = r.lgen.load(Ordering::Acquire).wrapping_add(1) & 0x7fff_ffff;
            r.lgen.store(lgen, Ordering::Release);
            self.subscribe(s, u, lgen, &cx.log);
            let (ok, wait) = self.recheck_chunked(s, u, &mut cx.log, false);
            if !ok {
                cx.wait = wait;
                return false;
            }
        }
        r.clear(super::F_PREDREF);
        if cx.log.iter().any(|p| matches!(p, Pred::Def { proj, .. } if *proj != 0)) {
            r.set(super::F_PROJREAD);
        } else {
            r.clear(super::F_PROJREAD);
        }
        if pread {
            r.set(super::F_PREDREAD);
        } else {
            r.clear(super::F_PREDREAD);
        }
        let predicted = r.flags.fetch_and(!super::F_PREDRUN, Ordering::AcqRel) & super::F_PREDRUN != 0;
        let guessed = predicted || cx.log.iter().any(|p| matches!(p, Pred::Def { spec: Some(_), .. }));
        let had = if guessed {
            r.flags.fetch_or(super::F_GUESS, Ordering::AcqRel)
        } else {
            r.flags.fetch_and(!super::F_GUESS, Ordering::AcqRel)
        } & super::F_GUESS
            != 0;
        if guessed && !had {
            self.outstanding.fetch_add(1, Ordering::Relaxed);
        } else if !guessed && had {
            self.outstanding.fetch_sub(1, Ordering::Relaxed);
        }
        if guessed {
            // Each target still in work confirms its part when it is
            // done (its commit touches this run first if the value moved).
            let mut ts: Vec<Slot> = cx
                .log
                .iter()
                .filter_map(|p| match p {
                    Pred::Def { spec: Some(q), .. } => Some(*q),
                    _ => None,
                })
                .collect();
            ts.sort_unstable();
            ts.dedup();
            let mut n = 0;
            for q in ts {
                let mut d = self.rec(q).data();
                if in_work(self.rec(q).st()) {
                    d.ex().guessed_by.push(s);
                    n += 1;
                }
            }
            r.data().ex().guess_pending = n;
            // A guessed read whose definer finished meanwhile (before this
            // run registered with it) is compared now: a value that moved
            // checks the run again.
            super::query::sib_cursor_reset();
            let moved = cx.log.iter().any(|p| {
                matches!(p, Pred::Def { spec: Some(q), .. } if !in_work(self.rec(*q).st()))
                    && !matches!(self.recheck(s, p, false), Ok(Ok(())))
            });
            if moved {
                r.set(super::F_STALE);
            }
            // (A run on its previous inputs holds until the evaluation's
            // end, unless one of them changes.)
            if n == 0 && !predicted && !moved {
                self.confirm_guess(s);
            }
        }
        self.stamp_depth(s, &cx.log, cx.run_ns);
        r.data().runs += 1;
        // Its read hints: the definers they name now (the critical
        // path orders it after them).
        let mut hint_waits: Vec<super::WaitRec> = Vec::new();
        let hints = r.data().stat.reads().cloned();
        if let Some(fs) = hints {
            for f in fs {
                // (A field it read orders it by that read.)
                if cx.log.iter().any(|p| matches!(p, Pred::Def { f: g, .. } if *g == f)) {
                    continue;
                }
                if let super::query::Q::Ans(o) = self.sread(u, || self.q_def(s, f, false, s))
                    && let DAns::Site { node, .. } = o.ans
                {
                    hint_waits.push(super::WaitRec {
                        kind: "hint",
                        by: node,
                        id: self.rec(node).id.load(Ordering::Acquire),
                        field: Some(f),
                        prefix: None,
                    });
                }
            }
        }
        // A claim of nothing and an effect of none, where the text declares
        // neither and the node holds neither, change nothing: dropped, so
        // they take no structural writer (which every reader of the unit
        // would retry behind).
        {
            let d = r.data();
            if cx.claim == Some(0)
                && d.stat.claim == ClaimS::None
                && d.x
                    .as_ref()
                    .is_none_or(|e| e.claim_run.is_none() && e.claimed.is_empty())
            {
                cx.claim = None;
            }
            if cx.effect == Some(crate::api::Effect::None)
                && d.stat.effect == crate::api::Effect::None
                && d.res
                    .as_ref()
                    .is_none_or(|c| matches!(c.effect, None | Some(crate::api::Effect::None)))
            {
                cx.effect = None;
            }
        }
        let unfold_changed;
        let structural = {
            let d = r.data();
            let same = |old: &[Slot], new: &[super::order::Spec]| {
                old.len() == new.len()
                    && old
                        .iter()
                        .zip(new)
                        .all(|(&o, n)| self.rec(o).id.load(Ordering::Acquire) == n.id())
            };
            unfold_changed =
                (!d.kids.is_empty() && !same(&d.kids, &cx.kids)) || (!d.conts.is_empty() && !same(&d.conts, &cx.conts));
            !same(&d.kids, &cx.kids)
                || !same(&d.conts, &cx.conts)
                || d.stat.claim == ClaimS::Pending
                // A run's claim over a declared one (or its end) moves
                // siblings: under the structure lock, as a pending claim.
                || cx.claim.is_some()
                || d.x.as_ref().is_some_and(|e| e.claim_run.is_some())
                || (cx.effect.is_some() && d.res.as_ref().is_none_or(|c| c.effect != cx.effect))
                || d.kind_val != cx.kind
                || d.kind_vals != cx.kinds
        };
        if structural {
            // The new children's static facts: their hooks read only
            // their texts, so they run before the structural lock.
            for sp in cx.kids.iter().chain(&cx.conts) {
                if let super::order::Spec::Node { id, op, args } = sp
                    && self.slot_of(*id).is_none()
                {
                    notes.pre.insert(*id, self.analyze_text(*op, args));
                }
            }
        }
        // Only new plain children (none before, no continuations, claims or
        // effects): a light writer, readers go on (see `UnitW`).
        let light = structural && {
            let d = r.data();
            d.kids.is_empty()
                && d.conts.is_empty()
                && cx.conts.is_empty()
                && d.stat.claim != ClaimS::Pending
                && cx.claim.is_none()
                && d.x.as_ref().is_none_or(|e| e.claim_run.is_none())
                && cx
                    .effect
                    .is_none_or(|e| d.res.as_ref().is_some_and(|c| c.effect == Some(e)))
                && d.kind_val == cx.kind
                && d.kind_vals == cx.kinds
                && cx.kids.iter().all(|sp| match sp {
                    // (Brackets known from the text: their frames are made
                    // before the entry is linked.)
                    super::order::Spec::Node { id, .. } => notes
                        .pre
                        .get(id)
                        .is_some_and(|st| st.effect != crate::api::Effect::Pending),
                    super::order::Spec::Marker { .. } => true,
                })
        };
        {
            let guard = if light {
                Ok(self.wunit_light(u))
            } else if structural {
                Ok(self.wunit(u))
            } else {
                Err(self.runit(u))
            };
            if r.st() != S_RUN || r.has(super::F_DOOM_FREE | super::F_DOOM_INERT) {
                drop(guard);
                return self.doomed(s) || r.st() != S_RUN;
            }
            // A run's claim: the siblings it saw must still be the first
            // `n` now (another claim may have landed since its reads were
            // checked, before this lock): else it runs again.
            if let Some(n) = cx.claim {
                let seen: Vec<Slot> = (0..n)
                    .filter_map(|i| {
                        cx.log.iter().find_map(|p| match p {
                            Pred::Sib { i: j, ans } if *j as usize == i => Some(ans.map(|a| a.0)),
                            _ => None,
                        })
                    })
                    .flatten()
                    .collect();
                if seen.len() == n && self.run_claim_list(s, n) != seen {
                    drop(guard);
                    return false;
                }
            }
            #[cfg(feature = "profile")]
            let held = std::time::Instant::now();
            self.snapshot(s);
            self.c.commits.fetch_add(1, Ordering::Relaxed);
            let limits = &self.limits;
            // Unfolding limits.
            let nkids = u32::try_from(cx.kids.len() + cx.conts.len()).unwrap_or(u32::MAX);
            let depth = self.depth(s);
            if (!cx.kids.is_empty() && depth + 1 > limits.unfold_depth) || nkids > limits.children_per_node {
                let created = nkids;
                let keep = created.min(limits.children_per_node.saturating_add(1));
                let last = cx
                    .kids
                    .iter()
                    .chain(&cx.conts)
                    .rev()
                    .take(keep.min(8) as usize)
                    .filter_map(|sp| match sp {
                        super::order::Spec::Node { op, args, .. } => Some((
                            self.op_name(*op).to_string(),
                            args.iter()
                                .map(|a| self.values.render(*a))
                                .collect::<Vec<_>>()
                                .join(" "),
                        )),
                        super::order::Spec::Marker { .. } => None,
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                self.fail(EvalError::RunawayCreation {
                    node: self.node_id(s),
                    depth: depth + 1,
                    created,
                    last,
                });
                return true;
            }
            let (old, stat_defs, claim_kind) = {
                let mut d = r.data();
                (d.res.take(), d.stat.defs.clone(), d.stat.claim)
            };
            let old = old.map_or_else(
                || Committed {
                    log: Vec::new(),
                    defs: Vec::new(),
                    contribs: Vec::new(),
                    pubs: Vec::new(),
                    effect: None,
                    output: ValueId::UNIT,
                    sig: 0,
                    binds: 0,
                    waits: Vec::new(),
                    read_ix: None,
                },
                |b| *b,
            );
            // Definitions (a run may make many thousands: sets, not scans).
            let declared: Option<crate::values::FxHashSet<Field>> =
                stat_defs.as_ref().map(|sd| sd.iter().map(|e| e.0).collect());
            // A static list is a guess too: a definition outside it widens
            // it (the insertion re-validates the field's readers after the
            // node, as a new definer's does).
            // The declaration is widened before the definitions go in: a
            // reader skips a busy node's entry its declaration does not
            // cover (`q_def`), and one that subscribed after the insertion
            // would not be told when the cover came.
            let widened: Vec<(Field, crate::api::Locality)> = declared.as_ref().map_or_else(Vec::new, |sd| {
                cx.defs
                    .iter()
                    .filter(|d| !sd.contains(&d.f))
                    .map(|d| (d.f, d.loc))
                    .collect()
            });
            if !widened.is_empty() {
                self.c.defs_refuted.fetch_add(widened.len() as u64, Ordering::Relaxed);
                let mut d = r.data();
                let st = &mut d.stat;
                if let Some(ds) = st.defs.as_mut() {
                    ds.extend(widened.iter().copied());
                }
                if let super::MayDefineS::Names(ns) = &mut st.may {
                    ns.extend(widened.iter().map(|w| w.0));
                }
            }
            if stat_defs.is_none() {
                // A definition outside the bound refutes it: the set is
                // unbounded from then on.
                let mut refuted = false;
                for d in &cx.defs {
                    if self.may_cover(s, d.f).is_none() {
                        if !refuted {
                            self.c.declarations_refuted.fetch_add(1, Ordering::Relaxed);
                        }
                        refuted = true;
                        let mut t = self.tal();
                        *t.refuted_by_field.entry(d.f).or_default() += 1;
                        *t.refuted_nodes.entry(s).or_default() += 1;
                    }
                }
                if refuted {
                    self.unregister_named(s);
                    r.data().stat.may = super::MayDefineS::Any;
                    r.set(super::F_WILD);
                }
            }
            for d in &cx.defs {
                self.insert_def(u, s, d.f, d.loc, d.val, &mut notes);
            }
            let made: crate::values::FxHashSet<Field> = cx.defs.iter().map(|d| d.f).collect();
            let defined = |f: Field| made.contains(&f);
            // Of a field it still defines, the kind (scope-local or not)
            // it no longer makes.
            let made_k: crate::values::FxHashSet<(Field, u8)> =
                cx.defs.iter().map(|d| (d.f, d.loc.entry_key())).collect();
            for d in &old.defs {
                let sc = d.loc.entry_key();
                if defined(d.f) && !made_k.contains(&(d.f, sc)) {
                    self.remove_def_k(u, s, d.f, sc, &mut notes);
                }
            }
            if let Some(sd) = &stat_defs {
                for &(f, loc) in sd {
                    let sc = loc.entry_key();
                    if !made_k.contains(&(f, sc)) {
                        self.remove_def_k(u, s, f, sc, &mut notes);
                    }
                }
            }
            for d in &old.defs {
                if !defined(d.f) && declared.as_ref().is_none_or(|sd| !sd.contains(&d.f)) {
                    self.remove_def(u, s, d.f, &mut notes);
                }
            }
            if stat_defs.is_none() && !cx.defs.is_empty() {
                self.c.pending_defining.fetch_add(1, Ordering::Relaxed);
            }
            // Contributions.
            for &(f, x) in &cx.contribs {
                self.set_contrib(u, s, f, Some(x), &mut notes);
            }
            for &(f, _) in &old.contribs {
                if !cx.contribs.iter().any(|c| c.0 == f) {
                    self.set_contrib(u, s, f, None, &mut notes);
                }
            }
            let declared = r.data().stat.contribs().cloned();
            if let Some(dc) = declared {
                for &(f, _) in &dc {
                    if !cx.contribs.iter().any(|c| c.0 == f) {
                        self.set_contrib(u, s, f, None, &mut notes);
                    }
                }
                let mut a = dc;
                let mut b = cx.contribs.clone();
                a.sort_by_key(|c| c.0.0.raw());
                b.sort_by_key(|c| c.0.0.raw());
                if a != b {
                    self.c.static_contribs_refuted.fetch_add(1, Ordering::Relaxed);
                }
            }
            // Publications.
            if !cx.pubs.is_empty() || !old.pubs.is_empty() {
                let mut p = lock(&self.pubs);
                let mut dirty = lock(&self.pubs_dirty);
                for &(f, _) in &old.pubs {
                    if let Some(m) = p.get_mut(&f) {
                        m.remove(&s);
                    }
                    dirty.insert(f);
                }
                for &(f, x) in &cx.pubs {
                    p.entry(f).or_default().insert(s, x);
                    dirty.insert(f);
                }
            }
            // Effect and kind.
            let stat_eff = r.data().stat.effect;
            let eff_changed = if stat_eff == crate::api::Effect::Pending {
                cx.effect.is_some() && old.effect != cx.effect
            } else {
                // Declared: the run's effect (if any) refutes it; one that
                // sets none goes back to the declaration.
                if cx.effect.is_some_and(|e| e != stat_eff) {
                    self.c.effects_refuted.fetch_add(1, Ordering::Relaxed);
                }
                old.effect.unwrap_or(stat_eff) != cx.effect.unwrap_or(stat_eff)
            };
            let kind_changed = {
                let mut d = r.data();
                let ch = d.kind_val != cx.kind || d.kind_vals != cx.kinds;
                d.kind_val = cx.kind;
                d.kind_vals = std::mem::take(&mut cx.kinds);
                ch
            };
            // The application's environment.
            if r.has(F_APP) && !cx.env.is_empty() {
                let au = r.data().app_unit();
                let mut e = self.unit(au).env.write();
                for &(f, x) in &cx.env {
                    self.note(super::merge::ke(f));
                    e.insert(f, x);
                }
            }
            // The result record (before the structure: brackets read it).
            let mut sigw: Vec<u64> = vec![u64::from(cx.op.0)];
            sigw.extend(cx.args.iter().map(|a| a.raw()));
            let mut bindw: Vec<u64> = Vec::new();
            for p in &cx.log {
                sigw.push(pred_value_hash(self, p));
                if let Pred::Def { ans, .. } = p {
                    bindw.push(match ans {
                        DAns::Site { node, .. } => self.rec(*node).id.load(Ordering::Acquire),
                        _ => 0,
                    });
                }
            }
            let waited = {
                let mut d = r.data();
                let w = d.x.as_mut().map_or_else(Vec::new, |e| {
                    e.wait_on = None;
                    e.ask = None;
                    std::mem::take(&mut e.waited)
                });
                d.trim_x();
                w
            };
            let res = Committed {
                log: {
                    // (Kept for the node's life: no growth slack.)
                    let mut l = std::mem::take(&mut cx.log);
                    l.shrink_to_fit();
                    l
                },
                defs: cx.defs.clone(),
                contribs: cx.contribs.clone(),
                pubs: cx.pubs.clone(),
                effect: if r.data().stat.effect == crate::api::Effect::Pending {
                    cx.effect.or(old.effect)
                } else {
                    cx.effect
                },
                output: out,
                sig: hash_words(5, &sigw),
                binds: hash_words(6, &bindw),
                // Only the waits the critical path orders by are kept.
                waits: waited
                    .into_iter()
                    .filter(|w| w.kind == "pending set")
                    .chain(hint_waits)
                    .collect(),
                read_ix: None,
            };
            let sens = res.log.iter().any(|p| match p {
                Pred::Def { sens, mode, .. } => *sens || *mode != DMode::Use,
                Pred::Kind { .. }
                | Pred::Sib { .. }
                | Pred::PrevSib { .. }
                | Pred::Claimed { .. }
                | Pred::Stream { .. }
                | Pred::Range { .. } => true,
                _ => false,
            });
            if sens {
                r.set(F_SENS);
            } else {
                r.clear(F_SENS);
            }
            // Subscriptions under the new log generation.
            {
                let mut d = r.data();
                d.diag = if cx.diag.is_empty() {
                    0
                } else {
                    let k = self.diag_next.fetch_add(1, Ordering::Relaxed);
                    let ds = std::mem::take(&mut cx.diag).into_boxed_slice();
                    self.diags.get_or_insert(k, || ds);
                    k
                };
                d.res = Some(Box::new(res));
                d.kept = None;
            }
            // Its reads may have seen through the subtree it replaces.
            restructured = eff_changed || kind_changed || unfold_changed;
            if restructured {
                notes.frames_from.push(s);
            }
            #[cfg(feature = "profile")]
            let st0 = std::time::Instant::now();
            // Unfolding (first: the old continuations' claims are released
            // before its own claim may take those siblings).
            let kids = std::mem::take(&mut cx.kids);
            let conts = std::mem::take(&mut cx.conts);
            let anchors = std::mem::take(&mut cx.anchors);
            #[cfg(feature = "profile")]
            let su0 = std::time::Instant::now();
            self.set_unfolding(s, &kids, &conts, &anchors, &mut notes);
            #[cfg(feature = "profile")]
            #[allow(clippy::cast_possible_truncation)]
            crate::obs::add_impl("commit_w_unfold", su0.elapsed().as_nanos() as u64);
            // Claims.
            if claim_kind == ClaimS::Pending {
                let n = cx.claim.unwrap_or(0);
                let mut list = Vec::new();
                if n > 0 {
                    self.following(s, s, |x, skipped| {
                        if !skipped {
                            list.push(x);
                        }
                        list.len() < n
                    });
                }
                // Its own view changes with what it claims: validate again.
                restructured |= self.apply_claim(s, &list, &mut notes);
            } else {
                // A declared claim is a guess: the run's claim, when it
                // made one, holds; one that makes none goes back to it.
                let prev = r.data().x.as_ref().and_then(|e| e.claim_run);
                match cx.claim {
                    Some(n) => {
                        r.data().ex().claim_run = Some(n);
                        r.set(super::F_SCLAIM);
                        let list = self.run_claim_list(s, n);
                        let before = r.data().claimed();
                        if list != before {
                            self.c.claims_refuted.fetch_add(1, Ordering::Relaxed);
                            let moved = list.iter().filter(|x| !before.contains(x)).count()
                                + before.iter().filter(|x| !list.contains(x)).count();
                            self.c.claim_siblings.fetch_add(moved as u64, Ordering::Relaxed);
                        }
                        restructured |= self.apply_claim(s, &list, &mut notes);
                    }
                    None if prev.is_some() => {
                        r.data().ex().claim_run = None;
                        if claim_kind == ClaimS::None {
                            r.clear(super::F_SCLAIM);
                            restructured |= self.apply_claim(s, &[], &mut notes);
                        } else {
                            notes.reclaim.push(s);
                        }
                    }
                    None => {}
                }
            }
            #[cfg(feature = "profile")]
            #[allow(clippy::cast_possible_truncation)]
            if structural {
                crate::obs::add_impl("commit_w_struct", st0.elapsed().as_nanos() as u64);
            }
            // Output.
            let old_out = r
                .has(F_HAS_OUT)
                .then(|| ValueId::from_raw(r.out.load(Ordering::Acquire)));
            let out_users = {
                let mut d = r.data();
                if old_out != Some(out) {
                    self.note(super::merge::kn(s));
                }
                r.out.store(out.raw(), Ordering::Release);
                r.set(F_HAS_OUT | F_DONE);
                r.clear(F_NEEDS_RUN);
                if r.has(super::F_MARKED) {
                    lock(&self.to_report).push(s);
                }
                if self.stepped() {
                    // Seen by the next step: busy until this one ends.
                    r.status.store(super::S_STEP, Ordering::Release);
                    lock(&self.step_done).push(s);
                } else {
                    // Out of the in-work lists while still running: a touch
                    // that finds it clean puts it back (left after, the
                    // touch's entry would be taken out again).
                    self.leave_work_lists(s);
                    r.status.store(S_CLEAN, Ordering::Release);
                }
                if old_out == Some(out) {
                    Vec::new()
                } else {
                    std::mem::take(&mut d.out_users)
                }
            };
            for (n, g) in out_users {
                notes.touch.push((n, g, Cause::Output { of: self.node_id(s) }));
            }
            waiters = std::mem::take(&mut r.data().waiters);
            #[cfg(feature = "profile")]
            let st1 = std::time::Instant::now();
            self.settle_structure(u, &mut notes);
            #[cfg(feature = "profile")]
            #[allow(clippy::cast_possible_truncation)]
            if structural {
                crate::obs::add_impl("commit_w_settle", st1.elapsed().as_nanos() as u64);
            }
            #[cfg(feature = "profile")]
            #[allow(clippy::cast_possible_truncation)]
            crate::obs::add_impl(
                if structural { "commit_hold_w" } else { "commit_hold_r" },
                held.elapsed().as_nanos() as u64,
            );
        }
        self.dispatch(&mut notes, s);
        // Claimed (or freed) while it committed: the mark came after the
        // check at the start, so it is acted on here.
        if r.has(super::F_DOOM_FREE | super::F_DOOM_INERT) && self.doomed(s) {
            for (n, q) in waiters {
                self.wake(n, q);
            }
            self.guesses_settle(s);
            return true;
        }
        if self.any_preds.load(Ordering::Relaxed) && !self.pred_open(s) {
            // Readers that took its predicted values (perhaps while an
            // earlier run's value stayed) are checked against these.
            self.settle_predictions(s);
        }
        // Against a touch that set the stale mark and then read the state
        // (still running): one of the two sees the other's write.
        std::sync::atomic::fence(Ordering::SeqCst);
        if restructured || r.has(super::F_STALE) {
            // Its own reads were made in the structure before its effect,
            // or something it read changed while it committed.
            r.clear(super::F_STALE);
            self.touch(s, u32::MAX, &Cause::Structure, false);
            // Back in work: its waiters wait for that check (they would
            // find it busy and stop).
            if in_work(r.st()) {
                let mut d = r.data();
                let mut w = waiters;
                w.append(&mut d.waiters);
                d.waiters = w;
                drop(d);
                if !in_work(r.st()) {
                    let w = std::mem::take(&mut r.data().waiters);
                    for (n, q) in w {
                        self.wake(n, q);
                    }
                    self.guesses_settle(s);
                }
                return true;
            }
        }
        self.wake_waiters_of(waiters);
        self.guesses_settle(s);
        true
    }

    /// The frames whose entries a read at `at` sees: its frame and its
    /// ancestors (none while its frame is unknown).
    pub(crate) fn scope_chain(&self, at: Slot) -> Vec<u32> {
        let mut v = Vec::new();
        let mut g = self.frame_of(at);
        while g != super::order::NO_FRAME && v.len() < 1 << 12 {
            v.push(g);
            g = self.frame_parent(g);
        }
        v
    }

    /// Subscribes a reader to the field's readers and to those of every
    /// frame of its scope chain.
    pub(crate) fn subscribe_reader(&self, idx: &mut super::FieldIdx, chain: &[u32], sub: Sub) {
        self.ins_sub(&mut idx.readers, sub);
        for &g in chain {
            self.ins_sub(idx.freaders.entry(g).or_default(), sub);
        }
    }

    /// Subscribes the reads of a committed log.
    pub(crate) fn subscribe(&self, s: Slot, u: u32, gn: u32, log: &[Pred]) {
        let mut fwd = false;
        for p in log {
            match p {
                Pred::Def {
                    f,
                    at,
                    ans,
                    sites,
                    mode,
                    ..
                } => {
                    if let DAns::Loop(_) = ans {
                        lock(&self.loop_readers).entry(*f).or_default().push((s, gn));
                        continue;
                    }
                    if let DAns::Env(_) = ans {
                        lock(self.env_readers_of(*f)).entry(*f).or_default().push((s, gn));
                    }
                    let sub = Sub {
                        node: s,
                        gn,
                        at: *at,
                        from: NIL,
                    };
                    let mut items = Vec::with_capacity(2);
                    if *mode == DMode::Use {
                        items.push(super::Inbox::Use {
                            sub,
                            chain: self.scope_chain(*at).into_boxed_slice(),
                        });
                    } else {
                        items.push(super::Inbox::Seq {
                            sub,
                            frame: self.frame_of(*at),
                        });
                    }
                    let one = match ans {
                        DAns::Site { node, .. } if *mode == DMode::Use => Some(*node),
                        _ => None,
                    };
                    for node in sites.iter().map(|x| x.0).chain(one) {
                        // (Both of its entries: the answer names the node.)
                        items.push(super::Inbox::User { node, reader: s, gn });
                    }
                    self.deliver(u, *f, items);
                }
                Pred::At { node, .. } => {
                    let xr = self.rec(*node);
                    let mut d = xr.data();
                    if xr.st() != S_FREE {
                        d.out_users.push((s, gn));
                    }
                }
                Pred::Out { node, .. } | Pred::App { node, .. } => {
                    let xr = self.rec(*node);
                    let mut d = xr.data();
                    if xr.st() != S_FREE {
                        d.out_users.push((s, gn));
                    }
                    if self.unit_of(*node) == u && self.before(s, *node) {
                        fwd = true;
                    }
                }
                Pred::Range { f, at, from, .. } => {
                    let a = self.anchor(*at, *from);
                    self.deliver(
                        u,
                        *f,
                        vec![super::Inbox::Range(Sub {
                            node: s,
                            gn,
                            at: *at,
                            from: a,
                        })],
                    );
                    if a != NIL {
                        self.sib_subscribe(a, s, gn);
                    }
                }
                Pred::Env { f, .. } => {
                    lock(self.env_readers_of(*f)).entry(*f).or_default().push((s, gn));
                }
                Pred::IsLoop { f, .. } => {
                    lock(&self.loop_readers).entry(*f).or_default().push((s, gn));
                }
                Pred::Kind { opener, .. } => {
                    if *opener != NIL {
                        self.sib_subscribe(*opener, s, gn);
                    }
                }
                Pred::Sib { ans, .. } => {
                    if let Some((x, _, _)) = ans {
                        self.sib_subscribe(*x, s, gn);
                    }
                    self.sib_subscribe(s, s, gn);
                }
                Pred::PrevSib { seen, .. } => {
                    for &(x, _, _) in seen {
                        self.sib_subscribe(x, s, gn);
                    }
                    self.sib_subscribe(s, s, gn);
                }
                Pred::Claimed { list } => {
                    for &(x, _, _) in list {
                        self.sib_subscribe(x, s, gn);
                    }
                }
                Pred::Stream { seen, reads, .. } => {
                    fwd = true;
                    for &(x, f) in reads {
                        lock(self.env_readers_of(f)).entry(f).or_default().push((s, gn));
                        if self.loops.contains_key(&f) {
                            lock(&self.loop_readers).entry(f).or_default().push((s, gn));
                        }
                        let site = match self.q_def(x, f, false, s) {
                            super::query::Q::Ans(o) => o.sites,
                            super::query::Q::Wait(_) => Vec::new(),
                        };
                        let chain = self.scope_chain(x);
                        let mut sh = self.wshard(u, f);
                        let idx = sh.entry((u, f)).or_default();
                        self.drain(idx);
                        self.subscribe_reader(
                            idx,
                            &chain,
                            Sub {
                                node: s,
                                gn,
                                at: x,
                                from: NIL,
                            },
                        );
                        for (q, _) in site {
                            let is: Vec<usize> = self.def_positions(idx, q).collect();
                            for i in is {
                                idx.defs[i].users.push((s, gn));
                            }
                        }
                    }
                    for &(x, _, _) in seen {
                        self.sib_subscribe(x, s, gn);
                        let xr = self.rec(x);
                        let mut d = xr.data();
                        if xr.st() != S_FREE {
                            d.out_users.push((s, gn));
                        }
                    }
                    self.sib_subscribe(s, s, gn);
                }
            }
        }
        if fwd {
            self.rec(s).set(F_FWD);
        } else {
            self.rec(s).clear(F_FWD);
        }
    }

    pub(crate) fn defs_of(&self, s: Slot) -> Vec<Def> {
        self.rec(s)
            .data()
            .res
            .as_ref()
            .map_or_else(Default::default, |c| c.defs.clone())
    }
}
