//! The commit of a run, on the worker that ran it: re-validate the reads
//! in the committed view under the unit's write lock, write the results,
//! subscribe the reads, and wake the waiters.

use std::sync::atomic::Ordering;

use super::ctx::Ctx;
use super::order::Notes;
use super::{
    ClaimS, Committed, DAns, DMode, Def, DefVal, F_APP, F_DONE, F_FWD, F_HAS_OUT, F_NEEDS_RUN, F_SENS, NIL, Pred,
    S_CLEAN, S_FREE, S_RUN, Shared, Slot, Sub, lock, wlock,
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
    }
}

impl Shared {
    /// Commits a finished run; `false`: a read broke, run again.
    pub(crate) fn commit(&self, cx: &mut Ctx<'_>, out: ValueId) -> bool {
        let s = cx.me;
        let u = cx.unit;
        let r = self.rec(s);
        crate::obs::span!(Finish);
        let mut notes = Notes::default();
        let waiters;
        let mut restructured;
        if r.st() != S_RUN || r.has(super::F_DOOM_FREE | super::F_DOOM_INERT) {
            return self.doomed(s) || r.st() != S_RUN;
        }
        // Subscribe first, then validate: a write that lands after the
        // subscription notifies this node (as stale: it validates again);
        // one that lands before it is seen by the validation.
        r.clear(super::F_STALE);
        if !self.sread(u, || {
            cx.log.iter_mut().all(|p| {
                let ok = matches!(self.recheck(s, p, false), Ok(Ok(())));
                if ok {
                    self.refresh_sites(s, p);
                }
                ok
            })
        }) {
            return false;
        }
        let lgen = r.lgen.load(Ordering::Acquire).wrapping_add(1) & 0x7fff_ffff;
        r.lgen.store(lgen, Ordering::Release);
        self.subscribe(s, u, lgen, &cx.log);
        if !self.sread(u, || {
            cx.log.iter().all(|p| matches!(self.recheck(s, p, false), Ok(Ok(()))))
        }) {
            return false;
        }
        let guessed = cx.log.iter().any(|p| matches!(p, Pred::Def { spec: Some(_), .. }));
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
                || (cx.effect.is_some() && d.res.as_ref().is_none_or(|c| c.effect != cx.effect))
                || d.kind_val != cx.kind
        };
        {
            let guard = if structural {
                Ok(self.wunit(u))
            } else {
                Err(super::rlock(&self.unit(u).s))
            };
            if r.st() != S_RUN || r.has(super::F_DOOM_FREE | super::F_DOOM_INERT) {
                drop(guard);
                return self.doomed(s) || r.st() != S_RUN;
            }
            self.snapshot(s);
            self.c.commits.fetch_add(1, Ordering::Relaxed);
            let limits = super::rlock(&self.limits).clone();
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
                },
                |b| *b,
            );
            // Definitions.
            for d in &cx.defs {
                self.insert_def(u, s, d.f, d.loc, d.val, &mut notes);
                if let Some(sd) = &stat_defs
                    && !sd.iter().any(|e| e.0 == d.f)
                {
                    lock(&self.undeclared).insert(s, d.f);
                }
            }
            let defined = |f: Field| cx.defs.iter().any(|d| d.f == f);
            {
                let mut un = lock(&self.undeclared);
                if un.get(&s).is_some_and(|&f| !defined(f)) {
                    un.remove(&s);
                }
            }
            if let Some(sd) = &stat_defs {
                for &(f, _) in sd {
                    if !defined(f) {
                        self.remove_def(u, s, f, &mut notes);
                    }
                }
            }
            for d in &old.defs {
                if !defined(d.f) && stat_defs.as_ref().is_none_or(|sd| !sd.iter().any(|e| e.0 == d.f)) {
                    self.remove_def(u, s, d.f, &mut notes);
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
                        let mut t = lock(&self.t);
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
            // Publications.
            if !cx.pubs.is_empty() || !old.pubs.is_empty() {
                let mut p = lock(&self.pubs);
                for &(f, _) in &old.pubs {
                    if let Some(m) = p.get_mut(&f) {
                        m.remove(&s);
                    }
                }
                for &(f, x) in &cx.pubs {
                    p.entry(f).or_default().insert(s, x);
                }
            }
            // Effect and kind.
            let eff_changed = cx.effect.is_some() && old.effect != cx.effect;
            let kind_changed = {
                let mut d = r.data();
                let ch = d.kind_val != cx.kind;
                d.kind_val = cx.kind;
                ch
            };
            // The application's environment.
            if r.has(F_APP) && !cx.env.is_empty() {
                let au = r.data().app_unit;
                let mut e = wlock(&self.unit(au).env);
                for &(f, x) in &cx.env {
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
            let waited = std::mem::take(&mut r.data().waited);
            let res = Committed {
                log: std::mem::take(&mut cx.log),
                defs: cx.defs.clone(),
                contribs: cx.contribs.clone(),
                pubs: cx.pubs.clone(),
                effect: cx.effect.or(old.effect),
                output: out,
                sig: hash_words(5, &sigw),
                binds: hash_words(6, &bindw),
                waits: waited,
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
            }
            // Unfolding.
            let kids = std::mem::take(&mut cx.kids);
            let conts = std::mem::take(&mut cx.conts);
            self.set_unfolding(s, &kids, &conts, &mut notes);
            // Output.
            let old_out = r
                .has(F_HAS_OUT)
                .then(|| ValueId::from_raw(r.out.load(Ordering::Acquire)));
            let out_users = {
                let mut d = r.data();
                r.out.store(out.raw(), Ordering::Release);
                r.set(F_HAS_OUT | F_DONE);
                r.clear(F_NEEDS_RUN);
                if self.stepped() {
                    r.status.store(super::S_STEP, Ordering::Release);
                    lock(&self.step_done).push(s);
                } else {
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
            if !self.stepped() {
                self.leave_work_lists(s);
            }
            waiters = std::mem::take(&mut r.data().waiters);
            self.settle_structure(u, &mut notes);
        }
        self.dispatch(&mut notes, s);
        if restructured || r.has(super::F_STALE) {
            // Its own reads were made in the structure before its effect,
            // or something it read changed while it committed.
            r.clear(super::F_STALE);
            self.touch(s, u32::MAX, &Cause::Structure, false);
        }
        for (n, q) in waiters {
            self.wake(n, q);
        }
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
                        lock(&self.env_readers).entry(*f).or_default().push((s, gn));
                    }
                    let chain = self.scope_chain(*at);
                    let mut sh = wlock(self.shard(u, *f));
                    let idx = sh.entry((u, *f)).or_default();
                    let sub = Sub {
                        node: s,
                        gn,
                        at: *at,
                        from: NIL,
                    };
                    if *mode == DMode::Use {
                        self.subscribe_reader(idx, &chain, sub);
                    } else {
                        self.ins_sub(&mut idx.readers, sub);
                        let fp = self.frame_of(*at);
                        self.ins_sub(idx.sreaders.entry(fp).or_default(), sub);
                    }
                    for &(node, _) in sites {
                        if let Some(i) = self.def_pos(idx, node) {
                            idx.defs[i].users.push((s, gn));
                        }
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
                    let mut sh = wlock(self.shard(u, *f));
                    let idx = sh.entry((u, *f)).or_default();
                    self.ins_sub(
                        &mut idx.range_readers,
                        Sub {
                            node: s,
                            gn,
                            at: *at,
                            from: a,
                        },
                    );
                    if a != NIL {
                        self.sib_subscribe(a, s, gn);
                    }
                }
                Pred::Env { f, .. } => {
                    lock(&self.env_readers).entry(*f).or_default().push((s, gn));
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
                        lock(&self.env_readers).entry(f).or_default().push((s, gn));
                        if self.loops.contains_key(&f) {
                            lock(&self.loop_readers).entry(f).or_default().push((s, gn));
                        }
                        let site = match self.q_def(x, f, false, s) {
                            super::query::Q::Ans(o) => o.sites,
                            super::query::Q::Wait(_) => Vec::new(),
                        };
                        let chain = self.scope_chain(x);
                        let mut sh = wlock(self.shard(u, f));
                        let idx = sh.entry((u, f)).or_default();
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
                            if let Some(i) = self.def_pos(idx, q) {
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
