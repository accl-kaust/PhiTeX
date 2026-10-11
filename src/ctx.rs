//! The interface an operation sees while it runs.

use std::sync::PoisonError;

use crate::engine::{
    AssumeKind, Effect, End, Engine, EvalError, K_FIELD, K_LOOP, K_OUTPUT, Locality, NIL, NodeId, OpId, RunResult,
    Slot, Use, eff_code,
};
use crate::hash::{FxHashMap, hash_words};
use crate::value::{Field, ValueId, Values};

/// Returned (via `?`) when a value is not available yet; the engine re-runs
/// the operation from the start once it is.
#[derive(Debug)]
pub struct Stop {
    _private: (),
}

/// The context of one run of one node. Everything an operation reads goes
/// through here and becomes a def-use edge.
pub struct Ctx<'a> {
    eng: &'a Engine,
    slot: Slot,
    spec: bool,
    fuel: u64,
    steps: u64,
    stopped: Option<End>,
    cache: FxHashMap<(u8, u64), Option<ValueId>>,
    uses: Vec<Use>,
    defs: Vec<(Field, ValueId, bool)>,
    children: Vec<(OpId, ValueId)>,
    env: Vec<(Field, ValueId)>,
    publishes: Vec<(Field, ValueId)>,
    contribs: Vec<(Field, ValueId)>,
    watch: Option<(Field, Slot, ValueId)>,
    effect: Option<u8>,
    claim: Option<usize>,
    assumptions: Vec<(Slot, AssumeKind, ValueId)>,
    sig: u64,
    error: Option<EvalError>,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(eng: &'a Engine, slot: Slot, spec: bool, fuel: u64) -> Self {
        Ctx {
            eng,
            slot,
            spec,
            fuel,
            steps: 0,
            stopped: None,
            cache: FxHashMap::default(),
            uses: Vec::new(),
            defs: Vec::new(),
            children: Vec::new(),
            env: Vec::new(),
            publishes: Vec::new(),
            contribs: Vec::new(),
            watch: None,
            effect: None,
            claim: None,
            assumptions: Vec::new(),
            sig: 0,
            error: None,
        }
    }

    /// The value table.
    #[must_use]
    pub fn values(&self) -> &'a Values {
        &self.eng.values
    }

    /// This node.
    #[must_use]
    pub fn node(&self) -> NodeId {
        self.eng.n(self.slot).id
    }

    /// The node's arguments (its text).
    #[must_use]
    pub fn args(&self) -> Vec<ValueId> {
        self.eng.args_vec(self.eng.n(self.slot).args)
    }

    /// Whether this run is speculative.
    #[must_use]
    pub fn is_speculative(&self) -> bool {
        self.spec
    }

    /// Whether `f` is a loop-back field.
    #[must_use]
    pub fn is_loopback(&self, f: Field) -> bool {
        self.eng.loop_vals.contains_key(&f)
    }

    pub(crate) fn stop<T>(&mut self, e: End) -> Result<T, Stop> {
        if self.stopped.is_none() {
            self.stopped = Some(e);
        }
        Err(Stop { _private: () })
    }

    fn check(&mut self) -> Result<(), Stop> {
        if self.stopped.is_some() {
            return Err(Stop { _private: () });
        }
        self.steps += 1;
        if self.spec && self.steps > self.fuel {
            return self.stop(End::OutOfFuel);
        }
        Ok(())
    }

    /// Consumes `n` steps of fuel (speculative runs stop when it runs out).
    ///
    /// # Errors
    ///
    /// [`Stop`] when a speculative run is out of fuel.
    pub fn tick(&mut self, n: u64) -> Result<(), Stop> {
        self.steps += n.saturating_sub(1);
        self.check()
    }

    fn sig_add(&mut self, kind: u8, key: u64, site: Slot, v: Option<ValueId>) {
        let sid = if site == NIL { 0 } else { self.eng.n(site).id.raw() };
        self.sig = self
            .sig
            .wrapping_add(hash_words(20, &[u64::from(kind), key, sid, v.map_or(0, ValueId::raw)]));
    }

    /// Reads a value of `t` if it is final, or (speculatively) if `t` has
    /// one at all; an in-work target becomes an assumption.
    fn read_node(&mut self, t: Slot, val: Option<ValueId>, kind: AssumeKind) -> Result<Option<ValueId>, Stop> {
        let e = self.eng;
        if e.settled(t) {
            return Ok(val);
        }
        let tn = e.n(t);
        if self.spec && tn.has_ran() {
            if tn.in_work() {
                self.assumptions.push((t, kind, val.unwrap_or(ValueId::NONE)));
            }
            return Ok(val);
        }
        if tn.in_work() {
            self.stop(End::Wait(t))
        } else {
            let h = tn.height + 1;
            self.stop(End::Raise(h))
        }
    }

    /// Uses field `f`: the value of its reaching definition (`None` if
    /// nothing defines it). Loop-back fields read the previous round.
    ///
    /// # Errors
    ///
    /// [`Stop`] if the value (or the structure the binding crosses) is not
    /// known yet.
    pub fn use_field(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        let e = self.eng;
        let key = f.0.raw();
        if self.is_loopback(f) {
            if let Some(&v) = self.cache.get(&(K_LOOP, key)) {
                return Ok(v);
            }
            let v = e.loop_vals.get(&f).copied();
            self.cache.insert((K_LOOP, key), v);
            self.uses.push(Use {
                key,
                site: NIL,
                kind: K_LOOP,
            });
            self.sig_add(K_LOOP, key, NIL, v);
            return Ok(v);
        }
        if let Some(&v) = self.cache.get(&(K_FIELD, key)) {
            return Ok(v);
        }
        let n = e.n(self.slot);
        if n.is_app() {
            self.cache.insert((K_FIELD, key), None);
            return Ok(None);
        }
        let w = e.walk(n.unit, n.label, f);
        let crossed = w.pending;
        if crossed != NIL && !self.spec {
            return self.stop(End::Wait(crossed));
        }
        let v = if w.site == NIL {
            e.units[n.unit as usize].env.get(&f).copied()
        } else {
            self.read_node(w.site, e.n(w.site).def_value(f), AssumeKind::Def(f))?
        };
        if crossed != NIL {
            // Predicted: the pending structure does not affect this use.
            self.assumptions
                .push((crossed, AssumeKind::Structure(f), v.unwrap_or(ValueId::NONE)));
        }
        self.cache.insert((K_FIELD, key), v);
        self.uses.push(Use {
            key,
            site: w.site,
            kind: K_FIELD,
        });
        self.sig_add(K_FIELD, key, w.site, v);
        Ok(v)
    }

    /// Defines `f` (with its declared locality; local if the definition set
    /// is dynamic).
    pub fn define(&mut self, f: Field, v: ValueId) {
        let n = self.eng.n(self.slot);
        let global = if n.is_dyn() {
            false
        } else {
            let Some(d) = n.defs.iter().find(|d| d.field == f) else {
                self.error = Some(EvalError::UndeclaredDefinition { node: n.id, field: f });
                return;
            };
            d.global
        };
        self.defs.retain(|d| d.0 != f);
        self.defs.push((f, v, global));
    }

    /// Defines `f` with an explicit locality (dynamic definition sets).
    pub fn define_with(&mut self, f: Field, v: ValueId, loc: Locality) {
        self.defs.retain(|d| d.0 != f);
        self.defs.push((f, v, loc == Locality::Global));
    }

    /// Publishes `v` as the next round's value of loop-back field `f`.
    pub fn publish(&mut self, f: Field, v: ValueId) {
        if let Some(p) = self.publishes.iter().find(|p| p.0 == f) {
            if p.1 != v {
                self.error = Some(EvalError::ConflictingPublication { field: f });
            }
            return;
        }
        self.publishes.push((f, v));
    }

    /// Contributes `v` to accumulator `f` at this node's position.
    pub fn contribute(&mut self, f: Field, v: ValueId) {
        self.contribs.push((f, v));
    }

    /// Unfolds into a child node (inline after this node and what it claims;
    /// for applications, into the body unit). Returns its index.
    pub fn child(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        let a = self.values().list(args);
        self.children.push((op, a));
        self.steps += 1;
        self.children.len() - 1
    }

    /// Unfolds a scope-opening marker.
    pub fn open_scope(&mut self) {
        let op = self.eng.open_op();
        self.child(op, Vec::new());
    }

    /// Unfolds a scope-closing marker.
    pub fn close_scope(&mut self) {
        let op = self.eng.close_op();
        self.child(op, Vec::new());
    }

    /// Resolves this node's pending scope effect.
    pub fn set_effect(&mut self, e: Effect) {
        self.effect = Some(eff_code(e));
    }

    /// Resolves this node's pending claim: it claims the next `n` siblings.
    pub fn claim(&mut self, n: usize) {
        self.claim = Some(n);
    }

    /// The text of the `i`-th following sibling.
    #[must_use]
    pub fn following(&self, i: usize) -> Option<(OpId, Vec<ValueId>)> {
        self.eng.following(self.slot).nth(i)
    }

    /// The text this node claims (its arguments from the program).
    #[must_use]
    pub fn claimed(&self) -> Vec<(OpId, Vec<ValueId>)> {
        let e = self.eng;
        let n = e.n(self.slot);
        if n.is_claim_dyn() {
            let k = self.claim.unwrap_or(0);
            return e.following(self.slot).take(k).collect();
        }
        e.aux_of(self.slot).map_or_else(Default::default, |a| {
            a.claimed
                .iter()
                .map(|&c| (e.n(c).op, e.args_vec(e.n(c).args)))
                .collect()
        })
    }

    /// Binds `f` at the start of this application's body unit.
    pub fn set_env(&mut self, f: Field, v: ValueId) {
        self.env.retain(|e| e.0 != f);
        self.env.push((f, v));
    }

    /// The output of child `i` of this run.
    ///
    /// # Errors
    ///
    /// [`Stop`] if the child has not produced it yet.
    pub fn output_of(&mut self, i: usize) -> Result<ValueId, Stop> {
        self.check()?;
        let e = self.eng;
        let ids = e.child_ids_pub(self.slot, &self.children[..=i]);
        let existing = e.children_of(self.slot);
        let ok = existing.len() > i && existing[..=i].iter().zip(&ids).all(|(&c, id)| e.n(c).id == *id);
        if !ok {
            return self.stop(End::NeedChild(i));
        }
        self.read_output(existing[i])
    }

    /// Applies `op` to `args` as a memoized lexical application (equal
    /// applications are one node) and returns its output.
    ///
    /// # Errors
    ///
    /// [`Stop`] if the application has not produced its output yet.
    pub fn apply(&mut self, op: OpId, args: Vec<ValueId>) -> Result<ValueId, Stop> {
        self.check()?;
        let a = self.values().list(args);
        match self.eng.memo.get(&(op, a)) {
            Some(&t) => self.read_output(t),
            None => self.stop(End::NeedApp(op, a)),
        }
    }

    fn read_output(&mut self, t: Slot) -> Result<ValueId, Stop> {
        let key = u64::from(t);
        if let Some(&v) = self.cache.get(&(K_OUTPUT, key)) {
            return Ok(v.unwrap_or(ValueId::UNIT));
        }
        let tn = self.eng.n(t);
        let val = tn.has_ran().then_some(tn.output);
        let v = self.read_node(t, val, AssumeKind::Output)?;
        self.cache.insert((K_OUTPUT, key), v);
        self.uses.push(Use {
            key,
            site: NIL,
            kind: K_OUTPUT,
        });
        self.sig_add(K_OUTPUT, self.eng.n(t).id.raw(), NIL, v);
        Ok(v.unwrap_or(ValueId::UNIT))
    }

    /// The fold of accumulator `f` over the nodes from `from` (exclusive of
    /// what precedes it; the unit start if `None`) up to this node. This node
    /// re-runs only when that range's fold (or its text) changes.
    ///
    /// # Errors
    ///
    /// [`Stop`] until every node in the range has finished.
    pub fn acc_range(&mut self, f: Field, from: Option<NodeId>) -> Result<ValueId, Stop> {
        self.check()?;
        let e = self.eng;
        let n = e.n(self.slot);
        let anchor = from.and_then(|id| e.slot_by_id(id)).unwrap_or(NIL);
        let lo = if anchor == NIL { 0 } else { e.n(anchor).label + 1 };
        let s = e.order_fold(n.unit, lo, n.label);
        if s.0 > 0 {
            return self.stop(End::WaitRange(f, anchor));
        }
        if s.1 >= e.level && !self.spec {
            return self.stop(End::Raise(s.1 + 1));
        }
        let v = e.acc_fold(n.unit, f, lo, n.label);
        self.watch = Some((f, anchor, v));
        let aid = if anchor == NIL { 0 } else { e.n(anchor).id.raw() };
        self.sig = self.sig.wrapping_add(hash_words(21, &[f.0.raw(), aid, v.raw()]));
        Ok(v)
    }

    /// The nearest preceding sibling whose text satisfies `pred`.
    #[must_use]
    pub fn prev_sibling_where(&self, pred: &dyn Fn(OpId, &[ValueId]) -> bool) -> Option<NodeId> {
        self.eng.prev_sibling_where(self.slot, pred)
    }

    /// Memoized text production: `produce` runs once per `(a, b)` (e.g. a
    /// function value and its argument text); every caller shares the
    /// produced value. `produce` must be pure.
    pub fn memo_text(&self, a: ValueId, b: ValueId, produce: impl FnOnce(&Values) -> ValueId) -> ValueId {
        let e = self.eng;
        if let Some(&v) = e.text_memo.read().unwrap_or_else(PoisonError::into_inner).get(&(a, b)) {
            return v;
        }
        let v = produce(&e.values);
        e.text_memo
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((a, b), v);
        v
    }

    pub(crate) fn finish(self, r: Result<ValueId, Stop>) -> RunResult {
        let n = self.eng.n(self.slot);
        let end = match (self.stopped, r) {
            (Some(e), _) => e,
            (None, Ok(v)) => End::Output(v),
            (None, Err(_)) => End::Unpredictable,
        };
        RunResult {
            node: n.id,
            slot: self.slot,
            epoch: n.epoch,
            speculative: self.spec,
            end,
            uses: self.uses,
            defs: self.defs,
            children: self.children,
            env: self.env,
            publishes: self.publishes,
            contribs: self.contribs,
            watch: self.watch,
            effect: self.effect,
            claim: self.claim,
            assumptions: self.assumptions,
            sig: self.sig,
            error: self.error,
        }
    }
}
