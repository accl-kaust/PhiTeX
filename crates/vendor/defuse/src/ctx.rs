//! The interface an operation sees while it runs.

use std::sync::PoisonError;

use crate::MayDefine;

use crate::engine::{
    AssumeKind, Effect, End, Engine, EvalError, K_AT, K_ATPOS, K_CHAIN, K_ENV, K_FIELD, K_LOOP, K_OUTPUT, Locality,
    NIL, NodeId, OpId, ProjId, RunResult, Slot, State, StreamRange, Suspended, Until, Use, WaitKind, WalkResult,
    eff_code, eff_kind,
};
use crate::hash::{FxHashMap, hash_words};
use crate::value::{Field, Value, ValueId, Values};

/// Returned (via `?`) when a value is not available yet; the engine re-runs
/// the operation from the start once it is.
#[derive(Debug)]
pub struct Stop {
    _private: (),
}

/// The context of one run of one node. Everything an operation reads goes
/// through here and becomes a def-use edge.
#[allow(clippy::struct_excessive_bools)]
pub struct Ctx<'a> {
    eng: &'a Engine,
    slot: Slot,
    spec: bool,
    fuel: u64,
    steps: u64,
    stopped: Option<End>,
    /// The level this run has to be re-done at: above the highest finished
    /// node it read that is not settled yet (0: none). The run goes on with
    /// the current values and is re-done once, not once per such read.
    raise: u32,
    /// A speculative read of a finished node not yet settled (its value
    /// may still change).
    read_unsettled: bool,
    /// A speculative read answered by a prediction.
    read_predicted: bool,
    raise_from: Slot,
    cache: FxHashMap<(u8, u64), Option<ValueId>>,
    /// Walks the gate did for this run (same node, same batch state).
    pub(crate) walks: Vec<(Field, WalkResult)>,
    /// `following`'s position: the index the next call likely asks for and
    /// the label of the sibling before it.
    following_cursor: std::cell::Cell<(usize, u64)>,
    /// The following siblings consulted (index, node, text), for a resume.
    following_seen: std::cell::RefCell<Vec<(usize, Slot, OpId, ValueId)>>,
    uses: Vec<Use>,
    defs: Vec<(Field, ValueId, bool)>,
    /// Field -> index in `defs`, built once the list is long (a run
    /// defining many fields would otherwise scan it per definition).
    def_ix: Option<FxHashMap<Field, usize>>,
    children: Vec<(OpId, ValueId)>,
    /// Continuations: nodes to go after this one at its own depth.
    follows: Vec<(OpId, ValueId)>,
    env: Vec<(Field, ValueId)>,
    /// Environment reads the provider answered ([`Engine::set_env_provider`]).
    env_found: Vec<(Field, Option<ValueId>)>,
    publishes: Vec<(Field, ValueId)>,
    contribs: Vec<(Field, ValueId)>,
    watch: Option<(Field, Slot, Slot, ValueId)>,
    /// The earliest node a backward sibling scan stopped at.
    scan: std::cell::Cell<Option<Slot>>,
    narrow: Option<MayDefine>,
    effect: Option<u8>,
    kind: Option<ValueId>,
    claim: Option<usize>,
    claim_seen: Vec<Slot>,
    stream: Option<StreamRange>,
    touch_seq: usize,
    relabels: u64,
    binding_epoch: u64,
    assumptions: Vec<(Slot, AssumeKind, ValueId)>,
    sig: u64,
    error: Option<EvalError>,
    diagnostics: Vec<ValueId>,
    /// The operation's resume token ([`Self::park`]); restored for a
    /// resumed attempt until [`Self::parked`] takes it.
    token: Option<Box<dyn std::any::Any + Send>>,
    resumed: bool,
    pub(crate) restarted: Option<&'static str>,
    /// Text memo hits, misses; application memo hits, misses.
    memo: [std::cell::Cell<u32>; 4],
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
            raise: 0,
            read_unsettled: false,
            read_predicted: false,
            raise_from: NIL,
            cache: FxHashMap::default(),
            walks: Vec::new(),
            following_cursor: std::cell::Cell::new((0, 0)),
            following_seen: std::cell::RefCell::new(Vec::new()),
            uses: Vec::new(),
            defs: Vec::new(),
            def_ix: None,
            children: Vec::new(),
            follows: Vec::new(),
            env: Vec::new(),
            env_found: Vec::new(),
            publishes: Vec::new(),
            contribs: Vec::new(),
            watch: None,
            scan: std::cell::Cell::new(None),
            narrow: None,
            effect: None,
            kind: None,
            claim: None,
            claim_seen: Vec::new(),
            stream: None,
            touch_seq: eng.touch_log.len(),
            relabels: eng.relabels,
            binding_epoch: eng.binding_epoch,
            assumptions: Vec::new(),
            memo: Default::default(),
            sig: 0,
            error: None,
            diagnostics: Vec::new(),
            token: None,
            resumed: false,
            restarted: None,
        }
    }

    /// A run resumed from the state its last attempt stopped with (every
    /// recorded read verified unchanged by the engine).
    pub(crate) fn resume(eng: &'a Engine, slot: Slot, fuel: u64, s: Suspended) -> Self {
        let mut cx = Ctx::new(eng, slot, false, fuel);
        cx.steps = s.steps;
        cx.cache = s.cache;
        cx.uses = s.uses;
        cx.defs = s.defs;
        cx.def_ix = None;
        cx.children = s.children;
        cx.follows = s.follows;
        cx.env = s.env;
        cx.publishes = s.publishes;
        cx.contribs = s.contribs;
        cx.effect = s.effect;
        cx.kind = s.kind;
        cx.diagnostics = s.diagnostics;
        cx.sig = s.sig;
        cx.following_seen = std::cell::RefCell::new(s.following);
        cx.narrow = s.narrow;
        cx.token = Some(s.token.into_inner().unwrap_or_else(PoisonError::into_inner));
        cx.resumed = true;
        cx
    }

    /// Hands the engine a token to give back if this run stops at a read
    /// and is attempted again: the engine then keeps the run's state (the
    /// reads so far with their values, the children, continuations,
    /// definitions, environment, publications, contributions and
    /// diagnostics made before the stop), verifies on the next attempt
    /// that every read still resolves to the same settled definition and
    /// value, and if so starts that attempt from the kept state with
    /// [`Self::parked`] returning the token: the operation continues at
    /// the read instead of re-executing what came before (which it must
    /// not do again: the children and definitions are already there).
    /// If any read changed, or the node's text did, or the node was re-run
    /// for a structural reason, the state and the token are dropped and
    /// the attempt starts from the top. A speculative run never parks.
    /// Calling it again replaces the token; a run that completes drops it.
    pub fn park(&mut self, token: Box<dyn std::any::Any + Send>) {
        self.token = Some(token);
    }

    /// The token of the attempt this run resumes, once (`None` for a run
    /// starting from the top). See [`Self::park`].
    pub fn parked(&mut self) -> Option<Box<dyn std::any::Any + Send>> {
        if self.resumed { self.token.take() } else { None }
    }

    /// For a resumed run that re-executes from the top anyway: forgets
    /// what the kept state made (children, continuations, definitions,
    /// environment, publications, contributions, diagnostics) so the
    /// re-execution makes them again, and keeps only the reads, which hit
    /// the cache. Nothing for a run starting from the top.
    pub fn restart(&mut self) {
        if !self.resumed {
            return;
        }
        self.children.clear();
        self.follows.clear();
        self.defs.clear();
        self.def_ix = None;
        self.env.clear();
        self.publishes.clear();
        self.contribs.clear();
        self.diagnostics.clear();
        self.effect = None;
        self.kind = None;
        // The signature holds the reads only, which stay.
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

    /// This node's operation.
    #[must_use]
    pub fn op(&self) -> OpId {
        self.eng.n(self.slot).op
    }

    /// Whether this run is speculative.
    #[must_use]
    pub fn is_speculative(&self) -> bool {
        self.spec
    }

    /// The values' epoch ([`Engine::values_epoch`]): every [`ValueId`] this
    /// run makes or reads stays valid while it is unchanged, so a cache of
    /// interned values keyed by `(epoch, content)` needs no pinning.
    #[must_use]
    pub fn values_epoch(&self) -> u64 {
        self.eng.values_epoch()
    }

    /// Emits a diagnostic (a value: a message, a record) as part of this
    /// run's result: readable with [`Engine::diagnostics`] once the node
    /// is done. A speculative run's diagnostics are held until the run is
    /// confirmed and dropped with it if it is refuted; a run that stops
    /// keeps none.
    pub fn diagnostic(&mut self, d: ValueId) {
        self.diagnostics.push(d);
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
        if self.spec
            && tn.in_work()
            && let AssumeKind::Def(f) | AssumeKind::Proj(f, _) = kind
            && let Some(p) = e.statics(tn.op, tn.args).predicted(f)
        {
            // Predicted by the text (ahead of its previous run's value):
            // run on it; the node's run confirms or refutes it like any
            // guess.
            self.read_predicted = true;
            self.assumptions.push((t, kind, assumed(e, kind, Some(p))));
            return Ok(Some(p));
        }
        if self.spec && tn.has_ran() {
            if tn.in_work() {
                // Its previous run's definition, as the prediction of the
                // run to come.
                if matches!(kind, AssumeKind::Def(_) | AssumeKind::Proj(..)) {
                    self.read_predicted = true;
                }
                self.assumptions.push((t, kind, assumed(e, kind, val)));
            } else {
                self.read_unsettled = true;
            }
            return Ok(val);
        }
        if tn.in_work() {
            let field = match kind {
                AssumeKind::Def(f) | AssumeKind::Proj(f, _) | AssumeKind::Structure(f) => Some(f),
                AssumeKind::Output => None,
            };
            self.stop(End::Wait(t, WaitKind::Value, field))
        } else {
            if tn.height + 1 > self.raise {
                self.raise = tn.height + 1;
                self.raise_from = t;
            }
            Ok(val)
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
        self.use_field_as(f, 0)
    }

    /// The definition of `f` reaching the position just after node `at`
    /// (what it unfolded or claims included): the value and its defining
    /// node (`None`: the unit's start value). `at` `None` is the unit's
    /// start: its start value. The run re-runs when the definition reaching
    /// that position changes (another one reaches it, or its value
    /// changes) or `at` goes. Waiting and guessing are as for a field read
    /// there. An `at` not earlier in this node's unit is an error
    /// ([`EvalError::BadPosition`]).
    ///
    /// # Errors
    ///
    /// [`Stop`] while the definition (or structure before it) is in work.
    pub fn use_field_at(&mut self, f: Field, at: Option<NodeId>) -> Result<Option<(ValueId, Option<NodeId>)>, Stop> {
        self.check()?;
        let e = self.eng;
        let n = e.n(self.slot);
        let key = f.0.raw();
        let Some(at_id) = at else {
            let v = e.start_value(n.unit, f);
            self.uses.push(Use {
                key,
                site: NIL,
                kind: K_AT,
                proj: 0,
            });
            self.uses.push(Use {
                key: NIL.pack(),
                site: NIL,
                kind: K_ATPOS,
                proj: 0,
            });
            self.sig_add(K_AT, key, NIL, v);
            return Ok(v.map(|v| (v, None)));
        };
        let a = match e.slot_by_id(at_id) {
            Some(a) if e.n(a).unit == n.unit && e.n(a).label < n.label => a,
            _ => {
                self.error = Some(EvalError::BadPosition { node: n.id, at: at_id });
                return Ok(None);
            }
        };
        let w = e.walk(n.unit, e.at_pos(self.slot, a), f);
        if w.pending != NIL {
            return self.stop(End::Wait(w.pending, w.pending_kind, Some(f)));
        }
        let v = if w.site == NIL {
            e.start_value(n.unit, f)
        } else {
            self.read_node(w.site, e.n(w.site).def_value(f), AssumeKind::Def(f))?
        };
        self.uses.push(Use {
            key,
            site: w.site,
            kind: K_AT,
            proj: 0,
        });
        self.uses.push(Use {
            key: a.pack(),
            site: NIL,
            kind: K_ATPOS,
            proj: 0,
        });
        self.sig_add(K_AT, key, w.site, v);
        let by = (w.site != NIL).then(|| e.n(w.site).id);
        Ok(v.map(|v| (v, by)))
    }

    /// Uses field `f` through projection `p`: returns the projected value
    /// (`None` if nothing defines it), and the run depends only on it. A
    /// change of the definition that projects equal leaves the run
    /// standing, and a guess on it holds while it projects equal. A run
    /// that also reads `f` in full depends on the full value.
    ///
    /// # Errors
    ///
    /// [`Stop`] as for [`Ctx::use_field`].
    pub fn use_field_proj(&mut self, f: Field, p: ProjId) -> Result<Option<ValueId>, Stop> {
        self.use_field_as(f, p.0)
    }

    /// A field read made in full (`proj` 0) or through a projection.
    fn use_field_as(&mut self, f: Field, proj: u16) -> Result<Option<ValueId>, Stop> {
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
                proj: 0,
            });
            self.sig_add(K_LOOP, key, NIL, v);
            return Ok(v);
        }
        if let Some(&v) = self.cache.get(&(K_FIELD, key)) {
            if let Some(i) = self.uses.iter().position(|u| u.kind == K_FIELD && u.key == key)
                && self.uses[i].proj != proj
                && self.uses[i].proj != 0
            {
                // Read again in full (or through another projection): the
                // run depends on the full value.
                self.unproject(i, v);
            }
            return Ok(e.project(proj, v));
        }
        let n = e.n(self.slot);
        if n.is_app() {
            self.cache.insert((K_FIELD, key), None);
            return Ok(None);
        }
        let w = self
            .walks
            .iter()
            .find(|x| x.0 == f)
            .map_or_else(|| e.walk(n.unit, n.label, f), |x| x.1);
        let crossed = w.pending;
        if crossed != NIL && !self.spec {
            return self.stop(End::Wait(crossed, w.pending_kind, Some(f)));
        }
        let kind = if proj == 0 {
            AssumeKind::Def(f)
        } else {
            AssumeKind::Proj(f, proj)
        };
        let v = if w.site == NIL {
            e.start_value(n.unit, f)
        } else {
            self.read_node(w.site, e.n(w.site).def_value(f), kind)?
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
            proj,
        });
        let pv = e.project(proj, v);
        self.sig_add(K_FIELD | proj_tag(proj), key, w.site, pv);
        Ok(pv)
    }

    /// Turns the projected read `self.uses[i]` (full value `v`) into a full
    /// one: its signature term and its guesses now use the full value.
    fn unproject(&mut self, i: usize, v: Option<ValueId>) {
        let e = self.eng;
        let u = self.uses[i];
        let f = Field(ValueId(u.key));
        let sid = if u.site == NIL { 0 } else { e.n(u.site).id.raw() };
        let pv = e.project(u.proj, v);
        self.sig = self.sig.wrapping_sub(hash_words(
            20,
            &[
                u64::from(K_FIELD | proj_tag(u.proj)),
                u.key,
                sid,
                pv.map_or(0, ValueId::raw),
            ],
        ));
        self.sig_add(K_FIELD, u.key, u.site, v);
        for a in &mut self.assumptions {
            if a.1 == AssumeKind::Proj(f, u.proj) {
                // The guess read `v` (the projection was taken from it).
                *a = (a.0, AssumeKind::Def(f), v.unwrap_or(ValueId::NONE));
            }
        }
        self.uses[i].proj = 0;
    }

    /// Reads the environment's `f` (what the client supplied with
    /// [`Engine::set_environment`]): `None` if it declared the field absent
    /// ([`Engine::set_environment_absent`]), and otherwise, until it does
    /// either, the node waits (the evaluation ends with it stalled, not
    /// as a cycle, and it runs again when the field arrives). Unlike
    /// [`Ctx::use_field`], a definition in the text does not shadow it.
    ///
    /// # Errors
    ///
    /// [`Stop`] while the field is neither supplied nor absent.
    pub fn use_env(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        let e = self.eng;
        let key = f.0.raw();
        if let Some(&v) = self.cache.get(&(K_ENV, key)) {
            return Ok(v);
        }
        let n = e.n(self.slot);
        let v = if let Ok(v) = e.env_value(n.unit, f) {
            v
        } else {
            // Not held (a root unit): the provider may answer now, on this
            // thread.
            let Some(v) = e.env_provider.as_ref().and_then(|p| p(&e.values, f)) else {
                return self.stop(End::WaitEnv(f));
            };
            self.env_found.push((f, v));
            v
        };
        self.cache.insert((K_ENV, key), v);
        self.uses.push(Use {
            key,
            site: NIL,
            kind: K_ENV,
            proj: 0,
        });
        self.sig_add(K_ENV, key, NIL, v);
        Ok(v)
    }

    /// Records a definition, replacing an earlier one of the same field.
    fn put_def(&mut self, f: Field, v: ValueId, global: bool) {
        const INDEX_FROM: usize = 32;
        if self.def_ix.is_none() && self.defs.len() >= INDEX_FROM {
            self.def_ix = Some(self.defs.iter().enumerate().map(|(i, d)| (d.0, i)).collect());
        }
        let at = match &self.def_ix {
            Some(ix) => ix.get(&f).copied(),
            None => self.defs.iter().position(|d| d.0 == f),
        };
        if let Some(i) = at {
            self.defs[i] = (f, v, global);
        } else {
            if let Some(ix) = &mut self.def_ix {
                ix.insert(f, self.defs.len());
            }
            self.defs.push((f, v, global));
        }
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
        self.put_def(f, v, global);
    }

    /// Defines `f` with an explicit locality (dynamic definition sets).
    pub fn define_with(&mut self, f: Field, v: ValueId, loc: Locality) {
        self.put_def(f, v, loc == Locality::Global);
    }

    // ------------------------------------------------------ contributions
    //
    // A node's contribution to a declared sequence is a hidden local
    // definition: `["c", element]`, or for a node that took the element
    // before it, `["t", replacement, has_replacement, visible]` where
    // `visible` is what later readers see as their previous element. Every
    // scope opener defines the hidden field inside its scope as the start
    // marker, so a reader's previous element is an ordinary binding within
    // its scope.

    fn seq_hidden(&self, seq: Field) -> Field {
        self.eng
            .seq_field(seq)
            .expect("sequence not declared (Engine::declare_sequence)")
    }

    /// Whether this node opens a scope before itself: its sequences start
    /// empty there. (A node with a pending effect decides it first.)
    fn at_own_start(&self) -> bool {
        self.eng.opens_before(self.slot, self.effect)
    }

    /// The element a hidden definition value shows to later readers.
    fn visible(&self, v: Option<ValueId>) -> Option<ValueId> {
        let v = v?;
        if v == self.eng.seq_start {
            return None;
        }
        let vals = self.values();
        let xs = match &*vals.get(v) {
            Value::List(xs) => xs.clone(),
            _ => return None,
        };
        match vals.as_str(xs[0]).as_deref() {
            Some("c") => Some(xs[1]),
            Some("t") => (xs[3] != self.eng.seq_start).then_some(xs[3]),
            _ => None,
        }
    }

    /// This node's contribution to sequence `seq` (one per node and
    /// sequence).
    pub fn push_contribution(&mut self, seq: Field, elem: ValueId) {
        let h = self.seq_hidden(seq);
        let v = self.values().list(vec![self.values().str("c"), elem]);
        self.put_def(h, v, false);
    }

    /// The contribution immediately before this node in its scope (`None`
    /// at the scope's start): a binding to the node that produced it.
    ///
    /// # Errors
    ///
    /// [`Stop`] until that node has run.
    pub fn prev_contribution(&mut self, seq: Field) -> Result<Option<ValueId>, Stop> {
        if self.at_own_start() {
            return Ok(None);
        }
        let h = self.seq_hidden(seq);
        let v = self.use_field(h)?;
        Ok(self.visible(v))
    }

    /// All contributions in this node's scope before it, in order, as one
    /// sequence value: each link of the chain of contributors back to the
    /// scope's start is a binding.
    ///
    /// # Errors
    ///
    /// [`Stop`] until every link has run.
    pub fn contributions_before_me(&mut self, seq: Field) -> Result<ValueId, Stop> {
        if self.at_own_start() {
            return Ok(ValueId::NIL);
        }
        let h = self.seq_hidden(seq);
        let mut val = self.use_field(h)?;
        let e = self.eng;
        let n = e.n(self.slot);
        let mut site = e.walk_site(n.unit, n.label, h).0;
        let vals = e.values();
        let mut rev = Vec::new();
        let mut skip = 0u64;
        while let Some(v) = val.filter(|&v| v != e.seq_start) {
            let xs = match &*vals.get(v) {
                Value::List(xs) => xs.clone(),
                _ => break,
            };
            if vals.as_str(xs[0]).as_deref() == Some("t") {
                if vals.as_bool(xs[2]) == Some(true) {
                    if skip > 0 {
                        skip -= 1;
                    } else {
                        rev.push(xs[1]);
                    }
                }
                skip += 1;
            } else if skip > 0 {
                skip -= 1;
            } else {
                rev.push(xs[1]);
            }
            if site == NIL {
                break;
            }
            let sn = e.n(site);
            let w = e.walk(sn.unit, sn.label, h);
            // A link crossing pending structure is not predicted.
            if w.pending != NIL {
                return self.stop(End::Wait(w.pending, w.pending_kind, Some(h)));
            }
            site = w.site;
            let key = h.0.raw();
            val = if site == NIL {
                None
            } else {
                self.read_node(site, e.n(site).def_value(h), AssumeKind::Def(h))?
            };
            self.uses.push(Use {
                key,
                site,
                kind: K_CHAIN,
                proj: 0,
            });
            self.sig_add(K_CHAIN, key, site, val);
        }
        rev.reverse();
        Ok(rev
            .into_iter()
            .fold(ValueId::NIL, |a, x| vals.seq_concat(a, vals.seq_unit(x))))
    }

    /// Takes away the contribution before this node (`replacement`: puts
    /// that in its place) for every later reader; returns what was taken.
    /// A definition: later readers bind to this node.
    ///
    /// # Errors
    ///
    /// As [`Ctx::contributions_before_me`].
    pub fn take_prev_contribution(
        &mut self,
        seq: Field,
        replacement: Option<ValueId>,
    ) -> Result<Option<ValueId>, Stop> {
        let h = self.seq_hidden(seq);
        let pre = self.contributions_before_me(seq)?;
        let vals = self.values();
        let mut xs = vals.seq_to_vec(pre);
        let taken = xs.pop();
        if let Some(r) = replacement {
            xs.push(r);
        }
        let visible = xs.last().copied().unwrap_or(self.eng.seq_start);
        let v = vals.list(vec![
            vals.str("t"),
            replacement.unwrap_or(ValueId::UNIT),
            vals.bool(replacement.is_some()),
            visible,
        ]);
        self.put_def(h, v, false);
        Ok(taken)
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

    /// Unfolds a continuation: a node placed right after this one (after
    /// its children) at this node's own depth, before its following
    /// siblings. It sees what those siblings see, including this node's
    /// definitions; it may claim and open scopes like a pushed sibling; it
    /// is made and freed with this node's run like a child. Continuations
    /// come after every child in text order, in the order they are
    /// unfolded; returns the position among them.
    pub fn follow(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        let a = self.values().list(args);
        self.follows.push((op, a));
        self.steps += 1;
        self.follows.len() - 1
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
        self.kind = eff_kind(e);
    }

    /// Sets the kind of the scope this node opens, for a static opener
    /// declared with `Brackets { kind_pending: true, .. }`: readers of
    /// [`Self::scope_kind`] inside it wait for this run, nothing else does.
    pub fn set_kind(&mut self, kind: ValueId) {
        self.kind = Some(kind);
    }

    /// Whether this node's scope effect is pending (decided by its run).
    #[must_use]
    pub fn effect_pending(&self) -> bool {
        self.eng.n(self.slot).has_dyn_effect()
    }

    /// The kind of this node's innermost enclosing scope (`None`: no kind,
    /// or at the unit's top level). A binding to that scope's opener: known
    /// once the opener's effect is.
    ///
    /// # Errors
    ///
    /// [`Stop`] while the opener is not settled.
    pub fn scope_kind(&mut self) -> Result<Option<ValueId>, Stop> {
        let f = self.eng.kind_field;
        Ok(self.use_field(f)?.filter(|&k| k != ValueId::UNIT))
    }

    /// A stream claim: the outputs of the nodes after this one, in text order
    /// after unfolding (a node that unfolded is replaced by its text), up to
    /// the first for which this node's [`Operation::until`] test fires (that
    /// node is not taken), or the end of the enclosing text. The nodes run as
    /// usual; this node waits only on the nodes up to that end, and on the
    /// definition each test reads at its candidate's position. A change in
    /// that range (a node, definition or scope effect coming, going or
    /// changing, or a taken output changing) runs this node again.
    ///
    /// # Errors
    ///
    /// [`Stop`] until each node up to the end has run, and each read
    /// definition is known.
    pub fn claim_stream(&mut self) -> Result<Vec<ValueId>, Stop> {
        self.check()?;
        let e = self.eng;
        let (unit, after_n, bound_n) = e.stream_bounds(self.slot);
        let after = e.n(after_n).label;
        let bound = if bound_n == NIL { u64::MAX } else { e.n(bound_n).label };
        let n = e.n(self.slot);
        let op = e.op(n.op).clone();
        let args = e.args_vec(n.args);
        let vals = e.values();
        let (open, close) = (e.open_op(), e.close_op());
        let mut outs = Vec::new();
        let mut label = after;
        let mut delim = bound_n;
        while let Some((l, c)) = e.order_next(unit, label).filter(|&(l, _)| l <= bound) {
            label = l;
            let cn = e.n(c);
            if cn.state == State::Inert || cn.is_app() {
                continue;
            }
            if self.spec && !cn.has_ran() {
                // Its unfolding is not known even approximately.
                return self.stop(End::Unpredictable);
            }
            if !e.children_of(c).is_empty() || cn.op == open || cn.op == close {
                self.read_output(c)?;
                continue;
            }
            let cargs = e.args_vec(cn.args);
            let fire = match op.until(&args, vals, cn.op, &cargs) {
                Until::No => false,
                Until::Yes => true,
                Until::Read(f) => {
                    let v = self.field_at_node(c, f)?;
                    op.until_value(&args, vals, cn.op, &cargs, v)
                }
                Until::ReadEach(fs) => {
                    let mut vs = Vec::with_capacity(fs.len());
                    for f in fs {
                        vs.push(self.field_at_node(c, f)?);
                    }
                    op.until_values(&args, vals, cn.op, &cargs, &vs)
                }
            };
            if fire {
                // The range runs to the node after the delimiter: its own
                // unfolding (should it unfold later) lies within. The
                // delimiter's output is not taken (nor waited for).
                delim = c;
                break;
            }
            outs.push(self.read_output(c)?);
        }
        self.stream = Some(StreamRange {
            after: after_n,
            end: delim,
        });
        self.sig = self.sig.wrapping_add(hash_words(22, &[after_n.pack(), delim.pack()]));
        Ok(outs)
    }

    /// The value field `f` has at node `at`'s position (a binding there;
    /// re-read on any change, like a contribution chain link).
    fn field_at_node(&mut self, at: Slot, f: Field) -> Result<Option<ValueId>, Stop> {
        let e = self.eng;
        let an = e.n(at);
        let w = e.walk(an.unit, an.label, f);
        if w.pending != NIL {
            return self.stop(End::Wait(w.pending, w.pending_kind, Some(f)));
        }
        let v = if w.site == NIL {
            e.start_value(an.unit, f)
        } else {
            self.read_node(w.site, e.n(w.site).def_value(f), AssumeKind::Def(f))?
        };
        let key = f.0.raw();
        self.uses.push(Use {
            key,
            site: w.site,
            kind: K_CHAIN,
            proj: 0,
        });
        self.sig_add(K_CHAIN, key, w.site, v);
        Ok(v)
    }

    /// Resolves this node's pending claim: it claims the next `n` siblings.
    pub fn claim(&mut self, n: usize) {
        self.claim = Some(n);
        self.claim_seen = self.eng.following_slots(self.slot, n);
    }

    /// The text of the `i`-th following sibling.
    #[must_use]
    pub fn following(&self, i: usize) -> Option<(OpId, Vec<ValueId>)> {
        // A scan asks for 0, 1, 2, ...: continue from the last sibling
        // returned instead of walking from the start each time.
        let (next, label) = self.following_cursor.get();
        let mut it = self.eng.following(self.slot);
        let skip = if next <= i && next > 0 {
            it.label = label;
            i - next
        } else {
            i
        };
        let r = it.nth(skip);
        if let Some((op, _)) = &r {
            self.following_cursor.set((i + 1, it.label));
            if let Some(&slot) = it.slots.last() {
                let mut seen = self.following_seen.borrow_mut();
                if !seen.iter().any(|f| f.0 == i) {
                    seen.push((i, slot, *op, self.eng.n(slot).args));
                }
            }
        }
        r
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
        if let Some(&t) = self.eng.memo.get(&(op, a)) {
            self.memo[2].set(self.memo[2].get() + 1);
            self.read_output(t)
        } else {
            self.memo[3].set(self.memo[3].get() + 1);
            self.stop(End::NeedApp(op, a))
        }
    }

    fn read_output(&mut self, t: Slot) -> Result<ValueId, Stop> {
        let key = t.pack();
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
            proj: 0,
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
        self.acc_range_to(f, from, None)
    }

    /// The fold of accumulator `f` over the window `(from, to]`: the nodes
    /// after `from` (the unit start if `None`) up to and including `to`
    /// (this node, exclusive, if `None`; a `to` at or after this node ends
    /// the window here). This node re-runs only when a contribution inside
    /// the window changes, the window's structure changes, or `to` goes.
    ///
    /// # Errors
    ///
    /// [`Stop`] until every node in the window has finished.
    pub fn acc_range_to(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<ValueId, Stop> {
        self.check()?;
        let e = self.eng;
        let n = e.n(self.slot);
        let anchor = from.and_then(|id| e.slot_by_id(id)).unwrap_or(NIL);
        let end = to
            .and_then(|id| e.slot_by_id(id))
            .filter(|&t| e.n(t).unit == n.unit && e.n(t).label < n.label)
            .unwrap_or(NIL);
        let lo = if anchor == NIL { 0 } else { e.n(anchor).label + 1 };
        let hi = if end == NIL { n.label } else { e.n(end).label + 1 };
        let s = e.order_fold(n.unit, f, lo, hi);
        if s.0 > 0 {
            return self.stop(End::WaitRange(f, anchor, end));
        }
        if s.1 >= e.level && !self.spec && s.1 + 1 > self.raise {
            self.raise = s.1 + 1;
            self.raise_from = NIL;
        }
        let v = e.acc_fold(n.unit, f, lo, hi);
        self.watch = Some((f, anchor, end, v));
        let aid = if anchor == NIL { 0 } else { e.n(anchor).id.raw() };
        let tid = if end == NIL { 0 } else { e.n(end).id.raw() };
        self.sig = self.sig.wrapping_add(hash_words(21, &[f.0.raw(), aid, tid, v.raw()]));
        Ok(v)
    }

    /// Narrows this node's definition set to `m`, a subset of what its
    /// text declares ([`Operation::may_define`]), from what the run has
    /// read so far: readers waiting on the node for a field outside `m`
    /// stop waiting and bind past it. Applied with the run's result (a
    /// stop included); a change that re-runs the node restores the text's
    /// answer until it narrows again. Defining outside `m` refutes the
    /// declaration, as defining outside the text's answer does.
    pub fn narrow(&mut self, m: MayDefine) {
        self.narrow = Some(m);
    }

    /// The nearest preceding sibling whose text satisfies `pred`.
    #[must_use]
    ///
    /// The siblings passed over are read: a change to one (its text, its
    /// presence, or a claim taking it) re-runs this node.
    pub fn prev_sibling_where(&self, pred: &dyn Fn(OpId, &[ValueId]) -> bool) -> Option<NodeId> {
        let e = self.eng;
        let (found, stop) = e.prev_sibling_where(self.slot, pred);
        let key = |s: Slot| if s == NIL { 0 } else { e.n(s).label };
        if self.scan.get().is_none_or(|old| key(stop) < key(old)) {
            self.scan.set(Some(stop));
        }
        found
    }

    /// Memoized text production: `produce` runs once per `(a, b)` (e.g. a
    /// function value and its argument text); every caller shares the
    /// produced value. `produce` must be pure.
    pub fn memo_text(&self, a: ValueId, b: ValueId, produce: impl FnOnce(&Values) -> ValueId) -> ValueId {
        let e = self.eng;
        if let Some(&v) = e.text_memo.read().unwrap_or_else(PoisonError::into_inner).get(&(a, b)) {
            self.memo[0].set(self.memo[0].get() + 1);
            return v;
        }
        self.memo[1].set(self.memo[1].get() + 1);
        let v = produce(&e.values);
        e.text_memo
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((a, b), v);
        v
    }

    /// A non-speculative stop at a read with a token: the state the next
    /// attempt may resume from. Not for runs whose reads the engine does
    /// not re-check one by one (claims, ranges, chains).
    fn resumable(&self, end: &End) -> bool {
        !self.spec
            && self.token.is_some()
            && self.watch.is_none()
            && self.scan.get().is_none()
            && self.stream.is_none()
            && self.claim.is_none()
            && self.assumptions.is_empty()
            && self
                .uses
                .iter()
                .all(|u| matches!(u.kind, K_FIELD | K_OUTPUT | K_ENV | K_LOOP))
            && matches!(
                end,
                End::Wait(..)
                    | End::WaitEnv(_)
                    | End::NeedApp(..)
                    | End::NeedChild(_)
                    | End::Gated(..)
                    | End::GateRaise(..)
            )
    }

    pub(crate) fn finish(mut self, r: Result<ValueId, Stop>) -> RunResult {
        let n = self.eng.n(self.slot);
        let follows = self.follows.len();
        let end = match (self.stopped.take(), r) {
            (Some(e), _) => e,
            // A run that completed after reading a finished node not yet
            // settled keeps its result: apply lifts it above that node
            // (RunResult::raise), and a change there dirties it through
            // its uses, as for any applied result. Re-running it from the
            // start bought nothing.
            (None, Ok(v)) => End::Output(v),
            (None, Err(_)) => End::Unpredictable,
        };
        let suspended = if self.resumable(&end) {
            Some(Box::new(Suspended {
                uses: self.uses.clone(),
                cache: self.cache.clone(),
                defs: self.defs.clone(),
                children: self.children.clone(),
                follows: self.follows.clone(),
                env: self.env.clone(),
                publishes: self.publishes.clone(),
                contribs: self.contribs.clone(),
                effect: self.effect,
                kind: self.kind,
                diagnostics: self.diagnostics.clone(),
                sig: self.sig,
                steps: self.steps,
                following: self.following_seen.borrow().clone(),
                narrow: self.narrow.clone(),
                token: std::sync::Mutex::new(self.token.take().expect("a token")),
            }))
        } else {
            None
        };
        self.children.append(&mut self.follows);
        let child_heights = if matches!(end, End::Output(_)) && !n.is_app() {
            self.eng.child_heights(self.slot, &self.children)
        } else {
            Vec::new()
        };
        // Work the apply phase would do one node at a time, done here on
        // the run's thread: the children's ids, their statics (cached by
        // text), the sites of the node's previous uses for its snapshot.
        let (child_ids, old_sites) = if matches!(end, End::Output(_)) {
            for &(op, args) in &self.children {
                drop(self.eng.statics(op, args));
            }
            (
                self.eng.child_ids_pub(self.slot, &self.children),
                Some(self.eng.site_list_pub(self.slot)),
            )
        } else {
            (Vec::new(), None)
        };
        RunResult {
            child_heights,
            child_ids,
            old_sites,
            node: n.id,
            slot: self.slot,
            epoch: n.epoch,
            speculative: self.spec,
            end,
            raise: self.raise,
            read_unsettled: self.read_unsettled,
            read_predicted: self.read_predicted,
            raise_from: self.raise_from,
            uses: self.uses,
            defs: self.defs,
            children: self.children,
            follows,
            env: self.env,
            env_found: self.env_found,
            publishes: self.publishes,
            contrib_folds: (!self.contribs.is_empty()).then(|| self.eng.contrib_folds(&self.contribs)),
            contribs: self.contribs,
            watch: self.watch,
            scan: self.scan.get(),
            narrow: self.narrow.clone(),
            effect: self.effect,
            kind: self.kind,
            claim: self.claim,
            claim_seen: self.claim_seen,
            stream: self.stream,
            touch_seq: self.touch_seq,
            relabels: self.relabels,
            binding_epoch: self.binding_epoch,
            assumptions: self.assumptions,
            sig: self.sig,
            error: self.error,
            diagnostics: self.diagnostics,
            suspended,
            resumed: self.resumed,
            restarted: self.restarted,
            memo: self.memo.map(std::cell::Cell::into_inner),
        }
    }
}

/// The value a guess of `kind` records: projected for a projected read.
fn assumed(e: &Engine, kind: AssumeKind, v: Option<ValueId>) -> ValueId {
    let p = if let AssumeKind::Proj(_, p) = kind { p } else { 0 };
    e.project(p, v).unwrap_or(ValueId::NONE)
}

/// The signature tag of a projected read (0 for a full one).
pub(crate) fn proj_tag(p: u16) -> u8 {
    if p == 0 { 0 } else { 0x80 }
}
