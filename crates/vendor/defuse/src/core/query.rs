//! The predicates the store answers. Each query has two views: strict (a
//! subject in work makes the reader wait on it) and committed (the last
//! committed facts, for commit validation and speculation).

use std::sync::atomic::Ordering;

use super::order::NO_FRAME;
use super::{
    DAns, DMode, DefVal, F_HAS_OUT, F_PEFF, K_PENDING, K_VALUE, NIL, Pred, S_FREE, S_INERT, S_MARKER, Shared, Slot,
    rlock,
};
use crate::api::Until;
use crate::values::{Field, Value, ValueId};

/// What a reader waits on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Subj {
    Node(Slot),
    Env(Field),
    /// A child of the reader's own run, not yet made.
    Child(u64),
}

/// A wait.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Wait {
    pub on: Subj,
    pub field: Option<Field>,
    pub prefix: Option<String>,
    pub kind: &'static str,
    /// Whether a committed answer exists to speculate with.
    pub has_prev: bool,
}

pub(crate) type StreamAns = (Vec<ValueId>, Vec<(Slot, u32, u64)>, Vec<(Slot, Field)>);

pub(crate) enum Q<T> {
    Ans(T),
    Wait(Wait),
}

/// A definer answer with what it consulted.
pub(crate) struct DefOut {
    pub ans: DAns,
    pub sens: bool,
    pub sites: Vec<(Slot, u32)>,
    /// Answered with the committed value of a definer still in work.
    pub guess: Option<Slot>,
}

pub(crate) fn wait_node(n: Slot, field: Option<Field>, kind: &'static str, has_prev: bool) -> Wait {
    Wait {
        on: Subj::Node(n),
        field,
        prefix: None,
        kind,
        has_prev,
    }
}

/// See `Shared::optimistic_pset`.
const PSET_FEW: usize = 2;

thread_local! {
    /// Whether the run on this thread may guess, and the guesses left.
    static SPEC_RUN: std::cell::Cell<(bool, u64)> = const { std::cell::Cell::new((false, 0)) };
    /// Whether the run on this thread read a predicted definition.
    static PRED_READ: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Whether an operation's run is reading on this thread (predictions
    /// are for runs, never for checks).
    static IN_RUN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

thread_local! {
    /// The last sibling a log check's `Pred::Sib` found: (reader, index, slot).
    static SIB_CUR: std::cell::Cell<Option<(Slot, usize, Slot)>> = const { std::cell::Cell::new(None) };
}

/// Starts a log check: sibling reads walk from the start again.
pub(crate) fn sib_cursor_reset() {
    SIB_CUR.with(|c| c.set(None));
}

/// Two use answers that give the run the same value (the site may differ).
/// (A site's committed value and the environment's are both just the
/// value to the run.)
pub(crate) fn same_value(a: &DAns, b: &DAns) -> bool {
    fn val(x: &DAns) -> Option<Option<ValueId>> {
        match x {
            DAns::Site {
                val: DefVal::Val(v), ..
            } => Some(Some(*v)),
            DAns::Env(v) => Some(*v),
            _ => None,
        }
    }
    matches!((val(a), val(b)), (Some(p), Some(q)) if p == q)
}

impl Shared {
    /// The entries of a label-sorted list strictly between `lo` and `hi`.
    fn span<'x>(&self, xs: &'x [Slot], lo: u64, hi: u64) -> &'x [Slot] {
        let i = xs.partition_point(|&x| self.rec(x).lab() <= lo);
        let j = xs.partition_point(|&x| self.rec(x).lab() < hi).max(i);
        &xs[i..j]
    }

    /// Under `Speculation::All` with free-running workers, a read does not
    /// wait for an earlier pending set: it takes the committed answer and
    /// the set's commit re-runs it if it defines the field after all.
    /// Whether a read may take a busy definer's committed value: under
    /// speculation, while the evaluation's budget lasts.
    fn take_guess(&self) -> bool {
        let (ok, _) = SPEC_RUN.with(std::cell::Cell::get);
        if !ok {
            return false;
        }
        if self.outstanding.load(Ordering::Relaxed) >= self.max_outstanding.load(Ordering::Relaxed) {
            self.c.fuel_outs.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.c.speculations.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// `x` through projection `p` (non-zero).
    pub(crate) fn project(&self, p: u16, x: ValueId) -> ValueId {
        let v = &self.values;
        if p == 1 {
            use crate::values::Value;
            let empty = match &*v.get(x) {
                Value::Unit | Value::Nil => true,
                Value::Int(i) => *i == 0,
                Value::Str(s) => s.is_empty(),
                Value::List(l) => l.is_empty(),
                Value::Bool(_) | Value::Seq(_) => false,
            };
            return v.bool(empty);
        }
        (self.projs[usize::from(p) - 2])(x, v)
    }

    /// Two answers to a projected read that project equal (same site).
    pub(crate) fn proj_eq(&self, p: u16, a: &DAns, b: &DAns) -> bool {
        let val = |d: &DAns| match d {
            DAns::Site {
                val: DefVal::Val(x) | DefVal::Push(x) | DefVal::Repl(x),
                ..
            }
            | DAns::Loop(x) => Some(*x),
            DAns::Env(x) | DAns::Seq(x) => *x,
            DAns::Site { .. } => None,
        };
        let same_site = match (a, b) {
            (DAns::Site { node: n, gn: g, .. }, DAns::Site { node: m, gn: h, .. }) => n == m && g == h,
            _ => std::mem::discriminant(a) == std::mem::discriminant(b),
        };
        same_site
            && match (val(a), val(b)) {
                (Some(x), Some(y)) => x == y || self.project(p, x) == self.project(p, y),
                (None, None) => a == b,
                _ => false,
            }
    }

    /// The value `q`'s text predicts it defines for `f`.
    pub(crate) fn predicted(&self, q: Slot, f: Field) -> Option<ValueId> {
        let d = self.rec(q).data();
        d.stat.preds()?.iter().find(|p| p.0 == f).map(|p| p.1)
    }

    /// Whether `q` ran in this evaluation.
    fn ran_now(&self, q: Slot) -> bool {
        self.rec(q).data().run_epoch == self.epoch.load(Ordering::Relaxed)
    }

    /// Whether `q`'s committed run stands on a prediction not yet borne
    /// out (its definitions may still move).
    pub(crate) fn pred_open(&self, q: Slot) -> bool {
        let fl = self.rec(q).flags.load(Ordering::Acquire);
        fl & super::F_PREDREAD != 0 && fl & super::F_GUESS != 0
    }

    /// Whether a read may run on a predicted definition: under any
    /// speculation, while the evaluation's budget lasts.
    pub(crate) fn set_in_run(on: bool) {
        IN_RUN.with(|c| c.set(on));
    }

    fn take_prediction(&self) -> bool {
        if self.spec.load(Ordering::Relaxed) == 0 || !IN_RUN.with(std::cell::Cell::get) {
            return false;
        }
        if self.outstanding.load(Ordering::Relaxed) >= self.max_outstanding.load(Ordering::Relaxed) {
            self.c.fuel_outs.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.c.speculations.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Whether this thread's run read a predicted definition (cleared).
    pub(crate) fn take_pred_read() -> bool {
        PRED_READ.with(|c| c.replace(false))
    }

    /// One step of this thread's speculative run: past its fuel it
    /// guesses no more (a fuel-out), and waits as a plain run would.
    fn spend(&self) {
        SPEC_RUN.with(|c| {
            let (ok, left) = c.get();
            if ok {
                if left == 0 {
                    self.c.fuel_outs.fetch_add(1, Ordering::Relaxed);
                    c.set((false, 0));
                } else {
                    c.set((true, left - 1));
                }
            }
        });
    }

    /// Lets this thread's run guess (`on`), with the policy's fuel.
    pub(crate) fn set_spec_run(&self, on: bool) {
        let fuel = self.fuel.load(Ordering::Relaxed);
        SPEC_RUN.with(|c| c.set((on, fuel)));
        PRED_READ.with(|c| c.set(false));
    }

    /// Whether this thread's run may guess.
    pub(crate) fn spec_run() -> bool {
        SPEC_RUN.with(|c| c.get().0)
    }

    /// A pending set found by `pset_between` among at most `PSET_FEW`
    /// candidates in the reader's span (a definition set narrow enough to
    /// name the field, or a prefix few nodes declare) is likely to
    /// define the field: the read waits for it rather than guess past it.
    fn optimistic_pset(&self, cands: usize) -> bool {
        if self.spec.load(Ordering::Relaxed) == 2 && cands > PSET_FEW {
            self.c.speculations.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Whether in-work `x` has a committed run that made no contribution
    /// to `f` (it is taken to make none again: should it, its commit
    /// notifies the windows that hold it).
    fn checks_without(&self, x: Slot, f: Field) -> bool {
        let d = self.rec(x).data();
        d.res.as_ref().is_some_and(|c| c.contribs.iter().all(|e| e.0 != f))
    }

    /// Whether `n` (not the reader `me`) is in work.
    /// Whether in-work `n` waits only to check its committed log (not to
    /// run): a read of its committed value is likely to stand, and is
    /// taken as a guess the check confirms or refutes.
    pub(crate) fn only_checking(&self, n: Slot) -> bool {
        let r = self.rec(n);
        match r.st() {
            super::S_RUN => r.has(super::F_CHECKING),
            super::S_DIRTY | super::S_PARK => !r.has(super::F_NEEDS_RUN) && r.data().res.is_some(),
            _ => false,
        }
    }

    pub(crate) fn busy(&self, n: Slot, me: Slot) -> bool {
        n != me && self.busy_node(n)
    }

    /// An in-work pending-effect node strictly between labels `lo` and `hi`.
    fn peff_between(&self, u: u32, lo: u64, hi: u64, me: Slot) -> Option<Slot> {
        let list = self.unit(u).peff.read();
        if list.is_empty() {
            return None;
        }
        self.span(&list, lo, hi)
            .iter()
            .rev()
            .copied()
            .find(|&x| self.rec(x).has(super::F_IN_PEFF) && self.busy(x, me))
    }

    /// Whether a pending-set node may define `f`, and through which prefix.
    #[allow(clippy::option_option)]
    pub(crate) fn may_cover(&self, x: Slot, f: Field) -> Option<Option<String>> {
        let (op, args, hit) = {
            let d = self.rec(x).data();
            let hit = match &d.stat.may {
                super::MayDefineS::Nothing => None,
                super::MayDefineS::Any => Some(None),
                super::MayDefineS::Names(ns) => ns.contains(&f).then_some(None),
                super::MayDefineS::Prefixes(ps) => match &*self.values.get(f.0) {
                    Value::Str(name) => ps
                        .iter()
                        .find(|p| name.starts_with(p.as_str()))
                        .map(|p| Some(p.clone())),
                    _ => None,
                },
            };
            (d.op, d.args.clone(), hit)
        };
        let hit = hit?;
        if self.ops[op.0 as usize].may_define_field(&args, &self.values, f) {
            Some(hit)
        } else {
            None
        }
    }

    /// An in-work pending set strictly between `lo` and `hi` that may
    /// define `f` (named ones from the field index, wild ones from the
    /// unit's list), nearest to `hi` first.
    fn pset_between(
        &self,
        u: u32,
        named: Option<&[Slot]>,
        f: Field,
        lo: u64,
        hi: u64,
        me: Slot,
    ) -> Option<(Slot, Option<String>, usize)> {
        let mut best: Option<(u64, Slot, Option<String>, usize)> = None;
        if let Some(named) = named {
            let sp = self.span(named, lo, hi);
            for &x in sp.iter().rev() {
                if self.busy(x, me)
                    && let Some(p) = self.may_cover(x, f)
                {
                    best = Some((self.rec(x).lab(), x, p, sp.len()));
                    break;
                }
            }
        }
        let wild = self.unit(u).wild.read();
        if !wild.is_empty() {
            for &x in self.span(&wild, lo, hi).iter().rev() {
                let l = self.rec(x).lab();
                if best.as_ref().is_some_and(|b| b.0 >= l) {
                    break;
                }
                if self.rec(x).has(super::F_IN_WILD)
                    && self.busy(x, me)
                    && let Some(p) = self.may_cover(x, f)
                {
                    best = Some((l, x, p, usize::MAX));
                    break;
                }
            }
        }
        // Prefix pending sets: the lists of the prefixes of `f`'s name.
        // (Locks in the writers' order: the map, then the lengths.)
        let un = self.unit(u);
        let ver = un.pfx_ver.load(Ordering::Acquire);
        let cached = un.pfx_of.read().get(&f).filter(|e| e.0 == ver).map(|e| e.1.clone());
        let ps = cached.unwrap_or_else(|| {
            let ps: std::sync::Arc<[String]> = {
                let m = un.pfx.read();
                let lens = un.pfx_lens.read();
                match &*self.values.get(f.0) {
                    Value::Str(name) if !lens.is_empty() => lens
                        .iter()
                        .filter_map(|&n| name.get(..n))
                        .filter(|p| m.contains_key(*p))
                        .map(String::from)
                        .collect(),
                    _ => std::sync::Arc::from([]),
                }
            };
            un.pfx_of.write().insert(f, (ver, ps.clone()));
            ps
        });
        if !ps.is_empty() {
            let m = un.pfx.read();
            for p in ps.iter() {
                let Some(v) = m.get(p.as_str()) else { continue };
                let sp = self.span(v, lo, hi);
                for &x in sp.iter().rev() {
                    let l = self.rec(x).lab();
                    if best.as_ref().is_some_and(|b| b.0 >= l) {
                        break;
                    }
                    if self.rec(x).has(super::F_IN_WILD)
                        && self.busy(x, me)
                        && let Some(p) = self.may_cover(x, f)
                    {
                        best = Some((l, x, p, sp.len()));
                        break;
                    }
                }
            }
        }
        best.map(|b| (b.1, b.2, b.3))
    }

    /// The environment value a definer read falls back to.
    pub(crate) fn env_fallback(&self, u: u32, f: Field) -> Option<ValueId> {
        let un = self.unit(u);
        if un.app.load(Ordering::Acquire) != NIL {
            return un.env.read().get(&f).copied();
        }
        if let Some(&x) = un.env.read().get(&f) {
            return Some(x);
        }
        self.env.read().get(f).flatten()
    }

    /// The nearest earlier definition of `f` in scope at `at`.
    pub(crate) fn q_def(&self, at: Slot, f: Field, strict: bool, me: Slot) -> Q<DefOut> {
        self.spend();
        crate::obs::count!(Walk);
        let u = self.unit_of(at);
        let un = self.unit(u);
        let root = un.root_frame;
        let pl = self.rec(at).lab();
        let fp = self.frame_of(at);
        let sh = rlock(self.shard(u, f));
        let idx = sh.get(&(u, f));
        let mut sens = false;
        let mut site = None;
        let mut min_start: Option<u64> = None;
        if let Some(idx) = idx {
            // The candidates: the last entry before `at` in each frame of
            // its scope chain, and the last global one; the nearest wins.
            let mut chain: Vec<(&[Slot], usize, u32)> = Vec::new();
            let mut g = fp;
            while g != NO_FRAME && chain.len() < 1 << 12 {
                if let Some(v) = idx.local.get(&g) {
                    let i = v.partition_point(|&x| self.rec(x).lab() < pl);
                    chain.push((v.as_slice(), i, g));
                }
                g = self.frame_parent(g);
            }
            {
                let v = idx.globals.as_slice();
                chain.push((v, v.partition_point(|&x| self.rec(x).lab() < pl), NO_FRAME));
            }
            loop {
                crate::obs::count!(WalkLevel);
                let mut best: Option<(usize, u64)> = None;
                for (k, &(v, i, _)) in chain.iter().enumerate() {
                    if i > 0 {
                        let l = self.rec(v[i - 1]).lab();
                        if best.is_none_or(|b| l > b.1) {
                            best = Some((k, l));
                        }
                    }
                }
                let Some((k, _)) = best else { break };
                let q = chain[k].0[chain[k].1 - 1];
                chain[k].1 -= 1;
                if q == at {
                    continue;
                }
                if (q == me || self.busy(q, me)) && !self.may_define_now(q, f) {
                    // A definition its old text made: gone once it commits.
                    continue;
                }
                if let Some(i) = self.def_pos_at(idx, q, chain[k].2) {
                    site = Some(&idx.defs[i]);
                    break;
                }
            }
            // Scope sensitivity: local entries outside the root frame
            // between the answer and `at` (seen or not, a scope effect
            // could change what is seen).
            let lo_lab = site.map_or(0, |e| self.rec(e.node).lab());
            let v = &idx.nonroot;
            let i = v.partition_point(|&x| self.rec(x).lab() < lo_lab);
            if let Some(&x) = v.get(i)
                && self.rec(x).lab() < pl
            {
                sens = true;
                let mut fq = self.frame_of(x);
                let st = if fq == NO_FRAME {
                    0
                } else {
                    // The outermost frame below the root it is in.
                    let mut guard = 0;
                    while fq != root
                        && self.frame_parent(fq) != root
                        && self.frame_parent(fq) != NO_FRAME
                        && guard < 1 << 12
                    {
                        fq = self.frame_parent(fq);
                        guard += 1;
                    }
                    self.frame_start(fq)
                };
                min_start = Some(st);
            }
        }
        if strict {
            if let Some(lo) = min_start
                && let Some(x) = self.peff_between(u, lo, pl, me)
            {
                return Q::Wait(wait_node(x, Some(f), "scope effect", self.rec(x).has(super::F_DONE)));
            }
            let lo = site.map_or(0, |e| self.rec(e.node).lab());
            if let Some((x, prefix, cands)) = self.pset_between(u, idx.map(|i| i.named.as_slice()), f, lo, pl, me)
                && !self.optimistic_pset(cands)
            {
                return Q::Wait(Wait {
                    on: Subj::Node(x),
                    field: Some(f),
                    prefix,
                    kind: "pending set",
                    has_prev: self.rec(x).has(super::F_DONE),
                });
            }
        }
        if let Some(e) = site {
            let q = e.node;
            let mut guess = None;
            let mut val = e.val;
            // Refuted once this evaluation: reads only committed values
            // (waits where it would guess or predict).
            let cons = strict && self.conservative(me);
            let open = strict && q != me && self.any_preds.load(Ordering::Relaxed) && self.pred_open(q);
            if strict && (self.busy(q, me) || open) {
                if !cons
                    && let Some(x) = self.predicted(q, f)
                    // (Not once it committed another value: that one is
                    // what a guess takes now.)
                    && !matches!(e.val, DefVal::Val(v) if v != x)
                    && (self.rec(q).st() != super::S_STEP
                        || e.val == DefVal::Val(x)
                        || !self.rec(me).has(super::F_PREDREF))
                    && self.guess_ok(f)
                    && self.take_prediction()
                {
                    // A predicted definition: the reader runs on it, and
                    // the definer's commit compares by value.
                    PRED_READ.with(|c| c.set(true));
                    val = DefVal::Val(x);
                    guess = Some(q);
                } else if !self.busy(q, me) {
                    // Done on a guess of its own: read as committed.
                } else if e.val == DefVal::Pending
                    || cons
                    || !(self.guess_ok(f)
                        && (self.only_checking(q)
                            || (!self.rec(q).has(super::F_NEEDS_RUN | super::F_PREDRUN) && self.take_guess())))
                {
                    return Q::Wait(wait_node(q, Some(f), "definition", e.val != DefVal::Pending));
                } else {
                    guess = Some(q);
                }
            } else if strict
                && !cons
                && self.any_preds.load(Ordering::Relaxed)
                && !self.rec(me).has(super::F_PREDREF)
                && let Some(x) = self.predicted(q, f)
                && !matches!(e.val, DefVal::Val(v) if v != x)
                && (e.val == DefVal::Val(x) || self.ran_now(q))
                && self.guess_ok(f)
                && self.take_prediction()
            {
                // Read as a prediction whether or not the definer has
                // finished in this evaluation (what a run reads does not
                // hang on its timing): borne out, it does not order the
                // reader after the definer; refuted, the reader's commit
                // fails and it reads the value.
                PRED_READ.with(|c| c.set(true));
                val = DefVal::Val(x);
                guess = Some(q);
            }
            let gn = self.rec(q).gn.load(Ordering::Acquire);
            return Q::Ans(DefOut {
                ans: DAns::Site { node: q, gn, val },
                sens,
                sites: vec![(q, gn)],
                guess,
            });
        }
        drop(sh);
        Q::Ans(DefOut {
            ans: DAns::Env(self.env_fallback(u, f)),
            sens,
            sites: Vec::new(),
            guess: None,
        })
    }

    /// A sequence read in `at`'s innermost scope.
    pub(crate) fn q_seq(&self, at: Slot, f: Field, mode: DMode, strict: bool, me: Slot) -> Q<DefOut> {
        self.spend();
        let u = self.unit_of(at);
        let pl = self.rec(at).lab();
        let fp = self.frame_of(at);
        let start = if fp == NO_FRAME { 0 } else { self.frame_start(fp) };
        if strict && let Some(x) = self.peff_between(u, start, pl, me) {
            return Q::Wait(wait_node(x, Some(f), "scope effect", self.rec(x).has(super::F_DONE)));
        }
        let sh = rlock(self.shard(u, f));
        let idx = sh.get(&(u, f));
        if strict && let Some((x, prefix, _)) = self.pset_between(u, idx.map(|i| i.named.as_slice()), f, start, pl, me)
        {
            return Q::Wait(Wait {
                on: Subj::Node(x),
                field: Some(f),
                prefix,
                kind: "pending set",
                has_prev: self.rec(x).has(super::F_DONE),
            });
        }
        let mut marks: Vec<(Slot, DefVal)> = Vec::new();
        let mut sites = Vec::new();
        if let Some(idx) = idx
            && let Some(v) = idx.local.get(&fp)
        {
            let root = self.unit(u).root_frame;
            let hi = v.partition_point(|&x| self.rec(x).lab() < pl);
            let lo = if fp == root {
                0
            } else {
                v.partition_point(|&x| self.rec(x).lab() <= start)
            };
            for &q in v[lo.min(hi)..hi].iter().rev() {
                if q == at {
                    continue;
                }
                if (q == me || self.busy(q, me)) && !self.may_define_now(q, f) {
                    // A definition its old text made: gone once it commits.
                    continue;
                }
                let Some(i) = self.def_pos_at(idx, q, fp) else { continue };
                let e = &idx.defs[i];
                if strict && self.busy(q, me) {
                    return Q::Wait(wait_node(q, Some(f), "definition", e.val != DefVal::Pending));
                }
                sites.push((q, self.rec(q).gn.load(Ordering::Acquire)));
                marks.push((q, e.val));
            }
        }
        drop(sh);
        marks.reverse();
        let v = &self.values;
        let ans = match mode {
            DMode::Before | DMode::Use => {
                let mut list: Vec<ValueId> = Vec::new();
                for (_, m) in &marks {
                    match *m {
                        DefVal::Push(x) | DefVal::Val(x) => list.push(x),
                        DefVal::Take => {
                            list.pop();
                        }
                        DefVal::Repl(x) => {
                            list.pop();
                            list.push(x);
                        }
                        DefVal::Pending => {}
                    }
                }
                let s = list.iter().fold(ValueId::NIL, |a, &x| v.seq_concat(a, v.seq_unit(x)));
                DAns::Seq(Some(s))
            }
            DMode::Prev => {
                let mut k = 0usize;
                let mut got = None;
                for (_, m) in marks.iter().rev() {
                    match *m {
                        DefVal::Push(x) | DefVal::Val(x) => {
                            if k == 0 {
                                got = Some(x);
                                break;
                            }
                            k -= 1;
                        }
                        DefVal::Take => k += 1,
                        DefVal::Repl(x) => {
                            if k == 0 {
                                got = Some(x);
                                break;
                            }
                        }
                        DefVal::Pending => {}
                    }
                }
                DAns::Seq(got)
            }
        };
        Q::Ans(DefOut {
            ans,
            sens: true,
            sites,
            guess: None,
        })
    }

    /// The output of `n`.
    pub(crate) fn q_out(&self, n: Slot, strict: bool, me: Slot) -> Q<Option<ValueId>> {
        self.spend();
        let r = self.rec(n);
        if strict && self.busy(n, me) {
            return Q::Wait(wait_node(n, None, "output", r.has(F_HAS_OUT)));
        }
        if matches!(r.st(), S_FREE | S_INERT | S_MARKER) {
            return Q::Ans(None);
        }
        if r.has(F_HAS_OUT) {
            Q::Ans(Some(ValueId::from_raw(r.out.load(Ordering::Acquire))))
        } else {
            Q::Ans(None)
        }
    }

    /// The anchor slot of a range read (`NIL`: the unit's start).
    pub(crate) fn anchor(&self, at: Slot, from: Option<u64>) -> Slot {
        let Some(id) = from else { return NIL };
        let Some(s) = self.slot_of(id) else { return NIL };
        if self.unit_of(s) != self.unit_of(at) || matches!(self.rec(s).st(), S_FREE) {
            return NIL;
        }
        s
    }

    /// The fold of `f`'s contributions strictly between the anchor and `at`.
    #[inline(never)]
    pub(crate) fn q_range(
        &self,
        at: Slot,
        f: Field,
        from: Option<u64>,
        to: Option<u64>,
        strict: bool,
        me: Slot,
    ) -> Q<(ValueId, Slot)> {
        self.spend();
        let u = self.unit_of(at);
        let a = self.anchor(at, from);
        let lo = if a == NIL { 0 } else { self.rec(a).lab() };
        // The window ends before `at`, or just after `to` when that is
        // before it (a `to` gone: at `at`).
        let mut hi = self.rec(at).lab();
        if let Some(t) = to.and_then(|id| self.slot_of(id))
            && self.unit_of(t) == u
            && self.rec(t).st() != super::S_FREE
            && self.rec(t).lab() < hi
        {
            hi = self.rec(t).lab() + 1;
        }
        if strict {
            let mut x = if a == NIL {
                self.unit(u).first.load(Ordering::Acquire)
            } else {
                self.next(a)
            };
            while x != NIL && x != at {
                if self.rec(x).lab() >= hi {
                    break;
                }
                // (A node whose last run contributed nothing here is
                // passed: should it contribute, its commit notifies this
                // window.)
                if self.busy(x, me) && !self.contributes_statically(x, f) && !self.checks_without(x, f) {
                    return Q::Wait(Wait {
                        on: Subj::Node(x),
                        field: Some(f),
                        prefix: None,
                        kind: "range",
                        has_prev: true,
                    });
                }
                x = self.next(x);
            }
        }
        let Some(m) = self.accs.get(&f) else {
            return Q::Ans((ValueId::UNIT, NIL));
        };
        let v = &self.values;
        let sh = rlock(self.shard(u, f));
        let mut acc = m.empty(v);
        let mut last = NIL;
        if let Some(idx) = sh.get(&(u, f)) {
            let i = idx.contribs.partition_point(|c| self.rec(c.0).lab() <= lo);
            for &(s, x) in &idx.contribs[i..] {
                if self.rec(s).lab() >= hi {
                    break;
                }
                acc = m.combine(v, acc, x);
                last = s;
            }
        }
        Q::Ans((acc, last))
    }

    /// Whether `x`'s text declares its contribution to `f` (its run need
    /// not be waited for: the value is known). Per field: a node with
    /// static contributions to other fields is waited for as any other.
    fn contributes_statically(&self, x: Slot, f: Field) -> bool {
        self.rec(x).has(super::F_SCONTRIB)
            && self
                .rec(x)
                .data()
                .stat
                .contribs()
                .is_some_and(|c| c.iter().any(|e| e.0 == f))
    }

    /// The whole fold of `f` in unit `u`.
    pub(crate) fn acc_total(&self, u: u32, f: Field) -> Option<ValueId> {
        let m = self.accs.get(&f)?;
        let v = &self.values;
        let sh = rlock(self.shard(u, f));
        let mut acc = m.empty(v);
        if let Some(idx) = sh.get(&(u, f)) {
            for &(_, x) in &idx.contribs {
                acc = m.combine(v, acc, x);
            }
        }
        Some(acc)
    }

    /// The kind of the scope `at` is in.
    pub(crate) fn q_kind(&self, at: Slot, strict: bool, me: Slot) -> Q<(Option<ValueId>, Slot)> {
        self.spend();
        let u = self.unit_of(at);
        let own_pending = self.rec(at).has(F_PEFF);
        let fr = if own_pending {
            self.top_in(at)
        } else {
            self.frame_of(at)
        };
        if fr == NO_FRAME {
            return Q::Ans((None, NIL));
        }
        let start = self.frame_start(fr);
        let pl = self.rec(at).lab();
        if strict && let Some(x) = self.peff_between(u, start, pl, me) {
            return Q::Wait(wait_node(x, None, "scope effect", self.rec(x).has(super::F_DONE)));
        }
        let (km, kv, opener) = self.kind_of_frame(fr);
        match km {
            K_VALUE => Q::Ans((Some(kv), opener)),
            K_PENDING => {
                if strict && self.busy(opener, me) {
                    return Q::Wait(wait_node(opener, None, "kind", self.rec(opener).has(super::F_DONE)));
                }
                Q::Ans((self.opener_kind(opener, fr), opener))
            }
            _ => Q::Ans((None, opener)),
        }
    }

    /// The `i`-th following sibling (as `me` sees them) and every entry the
    /// walk passed.
    pub(crate) fn q_sib(&self, at: Slot, i: usize) -> (Option<(Slot, u32, u64)>, Vec<Slot>) {
        self.q_sib_from(at, at, 0, i)
    }

    /// `q_sib` resumed: the walk goes on after `from`, `at`'s sibling number
    /// `k0 - 1` (`from == at`, `k0 == 0`: from the start).
    pub(crate) fn q_sib_from(
        &self,
        at: Slot,
        from: Slot,
        k0: usize,
        i: usize,
    ) -> (Option<(Slot, u32, u64)>, Vec<Slot>) {
        let mut seen = Vec::new();
        let mut got = None;
        let mut k = k0;
        self.following_from(from, at, |x, skipped| {
            seen.push(x);
            if skipped {
                return true;
            }
            if k == i {
                got = Some(x);
                return false;
            }
            k += 1;
            true
        });
        if seen.is_empty() {
            seen.push(at);
        }
        (
            got.map(|x| (x, self.rec(x).gn.load(Ordering::Acquire), self.text_hash(x))),
            seen,
        )
    }

    /// Walks a stream claim's elements: `Ok((outputs, seen))`, or the wait.
    #[inline(never)]
    pub(crate) fn q_stream(&self, at: Slot, strict: bool, me: Slot) -> Q<StreamAns> {
        self.spend();
        let (op, args) = self.text(at);
        let o = self.ops[op.0 as usize].clone();
        let v = &self.values;
        let level = self.depth(at);
        let mut outs = Vec::new();
        let mut seen = Vec::new();
        let mut reads = Vec::new();
        let mut x = self.next(self.subtree_end(at));
        while x != NIL {
            let d = self.depth(x);
            if d < level {
                break;
            }
            let r = self.rec(x);
            let st = r.st();
            if st == S_MARKER {
                if d == level {
                    break;
                }
                x = self.next(x);
                continue;
            }
            if st == S_FREE || self.taken(x, me) || self.placed_after_view(x, me) {
                seen.push((x, r.gn.load(Ordering::Acquire), 1));
                x = self.next(x);
                continue;
            }
            if strict && self.busy(x, me) {
                // A delimiter by its text alone ends the stream whatever it unfolds.
                let (cop, cargs) = self.text(x);
                if matches!(o.until(&args, v, cop, &cargs), Until::Yes) {
                    seen.push((x, r.gn.load(Ordering::Acquire), self.text_hash(x) ^ 3));
                    break;
                }
                return Q::Wait(wait_node(x, None, "stream", r.has(F_HAS_OUT)));
            }
            if strict && self.reads_from(x, at) {
                // The element reads the claimer's own result.
                return Q::Wait(wait_node(x, None, "feeds itself", false));
            }
            let leaf = {
                let d = r.data();
                d.kids.is_empty()
            };
            let gn = r.gn.load(Ordering::Acquire);
            if !leaf {
                seen.push((x, gn, 2));
                x = self.next(x);
                continue;
            }
            let (cop, cargs) = self.text(x);
            let until = o.until(&args, v, cop, &cargs);
            if let Until::Read(f) = until {
                reads.push((x, f));
            }
            let ends = match until {
                Until::No => false,
                Until::Yes => true,
                Until::Read(f) => match self.read_at(x, f, strict, me) {
                    Q::Ans(val) => o.until_value(&args, v, cop, &cargs, val),
                    Q::Wait(w) => return Q::Wait(w),
                },
                Until::ReadEach(fs) => {
                    let mut vals = Vec::new();
                    for f in fs {
                        reads.push((x, f));
                        match self.read_at(x, f, strict, me) {
                            Q::Ans(val) => vals.push(val),
                            Q::Wait(w) => return Q::Wait(w),
                        }
                    }
                    o.until_values(&args, v, cop, &cargs, &vals)
                }
            };
            let h = self.text_hash(x);
            if ends {
                seen.push((x, gn, h ^ 3));
                break;
            }
            let out = if r.has(F_HAS_OUT) {
                ValueId::from_raw(r.out.load(Ordering::Acquire))
            } else {
                ValueId::UNIT
            };
            seen.push((x, gn, h ^ out.raw()));
            outs.push(out);
            x = self.next(x);
        }
        Q::Ans((outs, seen, reads))
    }

    /// Whether `x`'s committed run read a definition of `c` that `c`'s
    /// current text may still make (a definition its old text made is
    /// gone once `c` commits, and `x` runs again then).
    fn reads_from(&self, x: Slot, c: Slot) -> bool {
        let fields: Vec<Field> = {
            let d = self.rec(x).data();
            d.res.as_ref().map_or_else(Vec::new, |r| {
                r.log
                    .iter()
                    .filter_map(|p| match p {
                        Pred::Def {
                            f,
                            ans: DAns::Site { node, .. },
                            ..
                        } if *node == c => Some(*f),
                        _ => None,
                    })
                    .collect()
            })
        };
        if fields.is_empty() {
            return false;
        }
        fields.iter().any(|&f| self.may_define_now(c, f))
    }

    /// Whether `c`'s current text may define `f` (a definition entry it
    /// has may stem from an older text, until it commits again).
    pub(crate) fn may_define_now(&self, c: Slot, f: Field) -> bool {
        // The node's declaration as it stands (a run's definition outside
        // its static list widened it).
        let declared = self
            .rec(c)
            .data()
            .stat
            .defs
            .as_ref()
            .map(|ds| ds.iter().any(|d| d.0 == f));
        match declared {
            Some(b) => b,
            None => self.may_cover(c, f).is_some(),
        }
    }

    /// A field's value at entry `x` (before it), as `use_field` reads it.
    pub(crate) fn read_at(&self, x: Slot, f: Field, strict: bool, me: Slot) -> Q<Option<ValueId>> {
        if let Some(&lv) = self.loop_vals.read().get(&f) {
            return Q::Ans(Some(lv));
        }
        match self.q_def(x, f, strict, me) {
            Q::Ans(o) => Q::Ans(match o.ans {
                DAns::Site { val, .. } => match val {
                    DefVal::Val(v) | DefVal::Push(v) | DefVal::Repl(v) => Some(v),
                    _ => None,
                },
                DAns::Env(v) | DAns::Seq(v) => v,
                DAns::Loop(v) => Some(v),
            }),
            Q::Wait(w) => Q::Wait(w),
        }
    }

    /// The environment: `Ok(Some(answer))` known, `Ok(None)` unknown.
    #[allow(clippy::option_option)]
    pub(crate) fn q_env(&self, f: Field) -> Option<Option<ValueId>> {
        {
            let e = self.env.read();
            if let Some(a) = e.get(f) {
                return Some(a);
            }
            if e.unknown.contains(&f) {
                return None;
            }
        }
        let p = self.provider.as_ref()?;
        let Some(got) = p(&self.values, f) else {
            self.env.write().unknown.insert(f);
            return None;
        };
        self.env.write().provided.insert(f, got);
        self.c.env_provided.fetch_add(1, Ordering::Relaxed);
        Some(got)
    }

    /// Re-asks a recorded predicate: `Ok(true)` holds, `Ok(false)` broke
    /// (with a reason), or the wait.
    /// A read of a predicted definition whose definer is still in work:
    /// it stands until the definer's commit compares the values.
    fn holds_predicted(&self, me: Slot, p: &Pred, now: &DAns) -> bool {
        let Pred::Def {
            f,
            spec: Some(q),
            ans:
                DAns::Site {
                    node,
                    val: DefVal::Val(x),
                    ..
                },
            ..
        } = p
        else {
            return false;
        };
        // (Once the definer has committed another value, the prediction no
        // longer holds, whatever its own guesses' state.)
        matches!(now, DAns::Site { node: n, val, .. } if n == node
            && !matches!(val, DefVal::Val(v) if v != x))
            && node == q
            && (self.busy(*q, me) || self.pred_open(*q))
            && self.rec(*q).st() != super::S_STEP
            && self.predicted(*q, *f) == Some(*x)
    }

    pub(crate) fn recheck(&self, me: Slot, p: &Pred, strict: bool) -> Result<Result<(), &'static str>, Wait> {
        match p {
            Pred::Def {
                f, at, mode, ans, who, ..
            } => {
                if self.rec(*at).st() == S_FREE {
                    return Ok(Err("definition: gone"));
                }
                if let DAns::Loop(v) = ans {
                    return Ok(if self.loop_vals.read().get(f) == Some(v) {
                        Ok(())
                    } else {
                        Err("loop")
                    });
                }
                // A plain use of a field declared a loop-back since then
                // reads the round value now (as a run would).
                if *mode == DMode::Use && self.loop_vals.read().contains_key(f) {
                    return Ok(Err("loop: declared"));
                }
                let q = match mode {
                    DMode::Use => self.q_def(*at, *f, strict, me),
                    m => self.q_seq(*at, *f, *m, strict, me),
                };
                match q {
                    Q::Ans(o) => Ok(
                        if o.ans == *ans
                            || (!*who && *mode == DMode::Use && same_value(&o.ans, ans))
                            || self.holds_predicted(me, p, &o.ans)
                            || matches!(p, Pred::Def { proj, .. } if *proj != 0 && self.proj_eq(*proj, ans, &o.ans))
                        {
                            Ok(())
                        } else {
                            Err("definition: changed")
                        },
                    ),
                    Q::Wait(w) => Err(w),
                }
            }
            Pred::At { node, gn } => Ok(
                if self.rec(*node).gn.load(Ordering::Acquire) == *gn
                    && !matches!(self.rec(*node).st(), S_FREE | super::S_MARKER)
                {
                    Ok(())
                } else {
                    Err("position: gone")
                },
            ),
            Pred::Out { node, gn, val, .. } => {
                if self.rec(*node).gn.load(Ordering::Acquire) != *gn {
                    return Ok(Err("output: gone"));
                }
                match self.q_out(*node, strict, me) {
                    Q::Ans(o) => Ok(if o == *val { Ok(()) } else { Err("output: changed") }),
                    Q::Wait(w) => Err(w),
                }
            }
            Pred::App { node, gn, val } => {
                if self.rec(*node).gn.load(Ordering::Acquire) != *gn {
                    return Ok(Err("application: gone"));
                }
                match self.q_out(*node, strict, me) {
                    Q::Ans(o) => Ok(if o == Some(*val) {
                        Ok(())
                    } else {
                        Err("application: changed")
                    }),
                    Q::Wait(w) => Err(w),
                }
            }
            Pred::Range {
                f, at, from, to, val, ..
            } => match self.q_range(*at, *f, *from, *to, strict, me) {
                Q::Ans((v, _)) => Ok(if v == *val { Ok(()) } else { Err("range: changed") }),
                Q::Wait(w) => Err(w),
            },
            Pred::Env { f, val } => match self.q_env(*f) {
                Some(v) => Ok(if v == *val { Ok(()) } else { Err("environment: changed") }),
                None => Err(Wait {
                    on: Subj::Env(*f),
                    field: Some(*f),
                    prefix: None,
                    kind: "environment",
                    has_prev: false,
                }),
            },
            Pred::IsLoop { f, val } => Ok(if self.loops.contains_key(f) == *val {
                Ok(())
            } else {
                Err("loop-back: declared")
            }),
            Pred::Kind { at, val, .. } => match self.q_kind(*at, strict, me) {
                Q::Ans((v, _)) => Ok(if v == *val { Ok(()) } else { Err("kind: changed") }),
                Q::Wait(w) => Err(w),
            },
            Pred::Sib { i, ans } => {
                // A log's sibling reads come in order: one walk serves them
                // (the cursor is reset before each log is checked).
                let i = *i as usize;
                let (got, _) = match SIB_CUR.with(std::cell::Cell::get) {
                    Some((m, j, x)) if m == me && j < i => self.q_sib_from(me, x, j + 1, i),
                    _ => self.q_sib(me, i),
                };
                SIB_CUR.with(|c| c.set(got.map(|g| (me, i, g.0))));
                Ok(match (got, ans) {
                    (a, b) if a == *b => Ok(()),
                    (Some(a), Some(b)) if a.0 == b.0 && a.1 == b.1 => Err("following: changed"),
                    _ => Err("following: gone"),
                })
            }
            Pred::PrevSib { seen, .. } => {
                let mut cur = Vec::new();
                let want = seen.len();
                self.preceding(me, |x, skipped| {
                    cur.push((
                        x,
                        self.rec(x).gn.load(Ordering::Acquire),
                        self.text_hash(x) ^ u64::from(skipped),
                    ));
                    cur.len() < want
                });
                Ok(if cur == *seen && cur.len() == want {
                    Ok(())
                } else if cur.len() > want || cur[..cur.len().min(want)] != seen[..cur.len().min(want)] {
                    Err("preceding: changed")
                } else {
                    // The walk now stops earlier or reaches further.
                    Err("preceding: changed")
                })
            }
            Pred::Claimed { list } => {
                let ok = list
                    .iter()
                    .all(|&(x, g, h)| self.rec(x).gn.load(Ordering::Acquire) == g && self.text_hash(x) == h);
                let cur = self.rec(me).data().claimed();
                let same = cur.len() == list.len() && cur.iter().zip(list).all(|(a, b)| *a == b.0);
                Ok(if ok && same { Ok(()) } else { Err("claimed: changed") })
            }
            Pred::Stream { seen, outs, .. } => match self.q_stream(me, strict, me) {
                Q::Ans((o, s, _)) if o == *outs && s == *seen => Ok(Ok(())),
                // The committed view differs where the run's strict walk
                // ended at an element in work (a delimiter by its text):
                // the claimer waits for that element instead of running
                // again while it is in work.
                Q::Ans(_) if !strict => match self.q_stream(me, true, me) {
                    Q::Ans((o, s, _)) if o == *outs && s == *seen => match s.last() {
                        // Only on an element that runs: one parked may wait,
                        // through others, on this claimer.
                        Some(&(x, _, _))
                            if self.busy(x, me)
                                && matches!(self.rec(x).st(), super::S_NEW | super::S_DIRTY | super::S_RUN) =>
                        {
                            Err(wait_node(x, None, "stream", false))
                        }
                        _ => Ok(Err("stream: changed")),
                    },
                    Q::Wait(w) => Err(w),
                    Q::Ans(_) => Ok(Err("stream: changed")),
                },
                Q::Ans(_) => Ok(Err("stream: changed")),
                Q::Wait(w) => Err(w),
            },
        }
    }
}
