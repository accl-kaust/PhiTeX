//! What a run sees: `Ctx`, the operation's handle on the store. Every read
//! is a predicate asked in the strict view and recorded with its answer;
//! every write is buffered until the commit.

// The recording wrappers return what the call returned, unit included.
#![allow(clippy::let_unit_value, clippy::semicolon_if_nothing_returned)]

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::order::Spec;
use super::query::{Q, Subj, Wait};
use super::{DAns, DMode, Def, DefVal, F_PEFF, NIL, Pred, S_FREE, Shared, Slot, lock, rlock};
use crate::api::FieldAt;
use crate::api::{Effect, Locality, NodeId, OpId, Stop};
use crate::values::{Field, FxHashMap, ValueId, Values, hash_words};

/// A run's view of the store.
pub struct Ctx<'a> {
    pub(crate) sh: &'a Shared,
    pub(crate) me: Slot,
    /// The calls of a recorded run, in order (`observe`).
    #[cfg(feature = "observe")]
    pub(crate) calls: std::cell::RefCell<Option<crate::record::Calls>>,
    pub(crate) unit: u32,
    pub(crate) id: u64,
    pub(crate) op: OpId,
    pub(crate) args: Arc<[ValueId]>,
    pub(crate) stat_defs: Option<Vec<(Field, Locality)>>,
    pub(crate) peff: bool,
    pub(crate) log: Vec<Pred>,
    /// Reads recorded through `&self` methods, merged into `log` when the
    /// run ends.
    pub(crate) side: std::cell::RefCell<Vec<Pred>>,
    pub(crate) defs: Vec<Def>,
    pub(crate) contribs: Vec<(Field, ValueId)>,
    pub(crate) pubs: Vec<(Field, ValueId)>,
    pub(crate) kids: Vec<Spec>,
    pub(crate) conts: Vec<Spec>,
    pub(crate) effect: Option<Effect>,
    pub(crate) kind: Option<ValueId>,
    /// Per opened frame (outermost first; `set_kinds`).
    pub(crate) kinds: Vec<ValueId>,
    pub(crate) claim: Option<usize>,
    /// The definition set this run narrowed its text's to.
    pub(crate) narrowed: Option<crate::api::MayDefine>,
    pub(crate) diag: Vec<ValueId>,
    pub(crate) env: Vec<(Field, ValueId)>,
    pub(crate) wait: Option<Wait>,
    pub(crate) token: Option<Box<dyn Any + Send>>,
    /// The last sibling `following` answered: (index, slot, generation).
    pub(crate) sib_cur: std::cell::Cell<Option<(usize, Slot, u32)>>,
    /// The sibling indexes already in the log (filled on first use).
    pub(crate) sib_logged: std::cell::RefCell<Option<crate::values::FxHashSet<u32>>>,
    /// How long the run took, in nanoseconds (0 on wasm32).
    pub(crate) run_ns: u64,
    pub(crate) kept: Option<Box<dyn Any + Send>>,
    pub(crate) restart: bool,
    pub(crate) dup: FxHashMap<u64, u32>,
    /// Where each field is in `defs`, once there are many.
    /// The run's definitions by field and whether scope-local.
    def_ix: FxHashMap<(Field, u8), usize>,
    pub(crate) static_claimed: Option<Vec<Slot>>,
    pub(crate) ask: Option<super::Ask>,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(sh: &'a Shared, me: Slot, kept: Option<Box<dyn Any + Send>>) -> Ctx<'a> {
        let r = sh.rec(me);
        let (op, args, stat_defs, static_claimed) = {
            let d = r.data();
            let sc = matches!(d.stat.claim, super::ClaimS::Count(_)).then(|| d.claimed());
            (d.op, d.args.clone(), d.stat.defs.clone(), sc)
        };
        Ctx {
            sh,
            #[cfg(feature = "observe")]
            calls: std::cell::RefCell::new(None),
            me,
            unit: sh.unit_of(me),
            id: r.id.load(Ordering::Acquire),
            op,
            args,
            stat_defs,
            peff: r.has(F_PEFF),
            log: Vec::new(),
            side: std::cell::RefCell::new(Vec::new()),
            defs: Vec::new(),
            contribs: Vec::new(),
            pubs: Vec::new(),
            kids: Vec::new(),
            conts: Vec::new(),
            effect: None,
            kind: None,
            kinds: Vec::new(),
            claim: None,
            narrowed: None,
            diag: Vec::new(),
            env: Vec::new(),
            wait: None,
            token: None,
            sib_cur: std::cell::Cell::new(None),
            sib_logged: std::cell::RefCell::new(None),
            run_ns: 0,
            kept,
            restart: false,
            dup: FxHashMap::default(),
            def_ix: FxHashMap::default(),
            static_claimed,
            ask: None,
        }
    }

    fn stop(&mut self, w: Wait) -> Stop {
        self.wait = Some(w);
        Stop { _private: () }
    }

    #[must_use]
    pub fn values(&self) -> &'a Values {
        &self.sh.values
    }

    #[must_use]
    pub fn args(&self) -> Vec<ValueId> {
        self.args.to_vec()
    }

    #[must_use]
    pub fn op(&self) -> OpId {
        self.op
    }

    #[must_use]
    pub fn node(&self) -> NodeId {
        NodeId(self.id)
    }

    /// The value of `f` reaching this node: the nearest earlier visible
    /// definition, a loop-back field's round value, or the environment.
    ///
    /// # Errors
    ///
    /// `Stop` when the answer's subject is still in work.
    pub fn use_field(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f,))
        } else {
            serde_json::Value::Null
        };
        let r = self.use_field_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_field", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_field_inner(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        Ok(self.read_def_at(self.me, f)?.map(|x| x.0))
    }

    /// The definition of `f` reaching the position just after node `at`
    /// (what it unfolded or claims included): the value and its defining
    /// node (`None`: the unit's start value). `at` `None` is the unit's
    /// start: its start value. The run re-runs when the definition reaching
    /// that position changes (another one reaches it, or its value
    /// changes) or `at` goes. Waiting and guessing are as for a field read
    /// there.
    ///
    /// An `at` that has gone (removed, or re-made by an edit) answers
    /// [`FieldAt::Gone`], and the run re-runs when it goes: whatever names
    /// the position gives the run the new node. No definition there is
    /// [`FieldAt::Unset`]. An `at` not earlier in this node's unit is an
    /// error ([`crate::EvalError::BadPosition`]).
    ///
    /// # Errors
    ///
    /// [`Stop`] while the definition (or structure before it) is in work.
    pub fn use_field_at(&mut self, f: Field, at: Option<NodeId>) -> Result<FieldAt, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f, &at))
        } else {
            serde_json::Value::Null
        };
        let r = self.use_field_at_inner(f, at);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_field_at", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_field_at_inner(&mut self, f: Field, at: Option<NodeId>) -> Result<FieldAt, Stop> {
        let sh = self.sh;
        let pos = match at {
            None => sh.unit(self.unit).first.load(std::sync::atomic::Ordering::Acquire),
            Some(n) => {
                let Some(a) = sh.slot_of(n.0) else {
                    return Ok(FieldAt::Gone);
                };
                let ar = sh.rec(a);
                if matches!(ar.st(), super::S_FREE | super::S_MARKER) {
                    return Ok(FieldAt::Gone);
                }
                if sh.unit_of(a) != self.unit || !sh.before(a, self.me) {
                    sh.fail(crate::api::EvalError::BadPosition {
                        node: self.node(),
                        at: n,
                    });
                    return Ok(FieldAt::Unset);
                }
                self.log.push(Pred::At {
                    node: a,
                    gn: ar.gn.load(std::sync::atomic::Ordering::Acquire),
                });
                // Just after its subtree: before the node that follows it.
                let nx = sh.next(sh.subtree_end(a));
                if nx == super::NIL { self.me } else { nx }
            }
        };
        if pos == super::NIL || pos == self.me && at.is_none() {
            return Ok(self.use_env(f)?.map_or(FieldAt::Unset, |x| FieldAt::Def(x, None)));
        }
        let r = self.read_def_at(pos, f)?;
        if pos != self.me {
            // (A re-ask at its own position would ask another place.)
            self.ask = None;
        }
        if let Some(Pred::Def { who, .. }) = self.log.last_mut() {
            *who = true;
        }
        let by = match self.log.last() {
            Some(Pred::Def {
                ans: DAns::Site { node, .. },
                ..
            }) => Some(sh.node_id(*node)),
            _ => None,
        };
        Ok(r.map_or(FieldAt::Unset, |x| FieldAt::Def(x.0, by)))
    }

    /// Reads `f` at the position of `at` (before it): the value, and the
    /// read is logged.
    fn read_def_at(&mut self, at: Slot, f: Field) -> Result<Option<(ValueId,)>, Stop> {
        if let Some(&lv) = self.sh.loop_vals.read().get(&f) {
            self.log.push(Pred::Def {
                f,
                at,
                mode: DMode::Use,
                ans: DAns::Loop(lv),
                sens: false,
                sites: Box::default(),
                spec: None,
                proj: 0,
                who: false,
            });
            return Ok(Some((lv,)));
        }
        let q = {
            let (sh, me) = (self.sh, self.me);
            sh.sread(self.unit, || sh.q_def(at, f, true, me))
        };
        match q {
            Q::Wait(w) => {
                self.ask = Some(super::Ask::Def(f));
                Err(self.stop(w))
            }
            Q::Ans(o) => {
                let v = match &o.ans {
                    DAns::Site { val, .. } => match *val {
                        DefVal::Val(x) | DefVal::Push(x) | DefVal::Repl(x) => Some(x),
                        _ => None,
                    },
                    DAns::Env(x) | DAns::Seq(x) => *x,
                    DAns::Loop(x) => Some(*x),
                };
                self.log.push(Pred::Def {
                    f,
                    at,
                    mode: DMode::Use,
                    ans: o.ans,
                    sens: o.sens,
                    sites: Box::default(),
                    spec: o.guess,
                    proj: 0,
                    who: false,
                });
                Ok(v.map(|x| (x,)))
            }
        }
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
    pub fn use_field_proj(&mut self, f: Field, p: crate::api::ProjId) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f, &p))
        } else {
            serde_json::Value::Null
        };
        let r = self.use_field_proj_inner(f, p);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_field_proj", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_field_proj_inner(&mut self, f: Field, p: crate::api::ProjId) -> Result<Option<ValueId>, Stop> {
        let v = self.use_field(f)?;
        if let Some(Pred::Def { proj, .. }) = self.log.last_mut() {
            *proj = p.0;
        }
        self.sh.any_proj.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(v.map(|x| self.sh.project(p.0, x)))
    }

    fn seq_read(&mut self, f: Field, mode: DMode) -> Result<Option<ValueId>, Stop> {
        let q = {
            let (sh, me) = (self.sh, self.me);
            sh.sread(self.unit, || sh.q_seq(me, f, mode, true, me))
        };
        match q {
            Q::Wait(w) => Err(self.stop(w)),
            Q::Ans(o) => {
                let v = match &o.ans {
                    DAns::Seq(x) => *x,
                    _ => None,
                };
                self.log.push(Pred::Def {
                    f,
                    at: self.me,
                    mode,
                    ans: o.ans,
                    sens: true,
                    sites: o.sites.into_boxed_slice(),
                    spec: o.guess,
                    proj: 0,
                    who: false,
                });
                Ok(v)
            }
        }
    }

    /// The environment's value of `f`; waits while it is neither supplied
    /// nor declared absent.
    ///
    /// # Errors
    ///
    /// `Stop` while the field is unknown.
    pub fn use_env(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f,))
        } else {
            serde_json::Value::Null
        };
        let r = self.use_env_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_env", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_env_inner(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        match self.sh.q_env(f) {
            Some(val) => {
                self.log.push(Pred::Env { f, val });
                Ok(val)
            }
            None => Err(self.stop(Wait {
                on: Subj::Env(f),
                field: Some(f),
                prefix: None,
                kind: "environment",
                has_prev: false,
            })),
        }
    }

    pub fn define(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f, &x))
        } else {
            serde_json::Value::Null
        };
        let r = self.define_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "define", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn define_inner(&mut self, f: Field, x: ValueId) {
        let loc = self
            .stat_defs
            .as_ref()
            .and_then(|d| d.iter().find(|e| e.0 == f).map(|e| e.1))
            .unwrap_or(Locality::Local);
        self.define_with(f, x, loc);
    }

    pub fn define_with(&mut self, f: Field, x: ValueId, loc: Locality) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f, &x, &loc))
        } else {
            serde_json::Value::Null
        };
        let r = self.define_with_inner(f, x, loc);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "define_with", a, serde_json::Value::Null);
        r
    }

    /// Defines `f` as a local of the scope this node opens after itself
    /// (`Brackets { opens >= 1, before: false }`): readers inside the scope
    /// see it, readers after its close see the node's other definitions.
    /// A node that opens no scope after itself defines it as a plain local.
    /// The same field may also be defined at the node's own level in the
    /// same run: that entry is seen after the scope closes.
    pub fn define_in_scope(&mut self, f: Field, x: ValueId) {
        self.define_with(f, x, Locality::Scope);
    }

    /// Defines `f` as a local of the `k`-th frame this node opens (0: the
    /// outermost): readers in that frame and the frames within it see it;
    /// after it closes, the node's own-level definition of `f` (if any).
    pub fn define_in_scope_at(&mut self, f: Field, k: u8, x: ValueId) {
        self.define_with(f, x, Locality::ScopeAt(k));
    }

    #[inline]
    fn define_with_inner(&mut self, f: Field, x: ValueId, loc: Locality) {
        self.set_def(f, loc, DefVal::Val(x));
    }

    fn set_def(&mut self, f: Field, loc: Locality, val: DefVal) {
        const SCAN: usize = 16;
        // A scope-local definition and an outer one of the same field are
        // two entries.
        let k = (f, loc.entry_key());
        let key = |d: &Def| (d.f, d.loc.entry_key());
        let at = if self.defs.len() <= SCAN {
            self.defs.iter().position(|d| key(d) == k)
        } else {
            if self.def_ix.is_empty() {
                self.def_ix = self.defs.iter().enumerate().map(|(i, d)| (key(d), i)).collect();
            }
            self.def_ix.get(&k).copied()
        };
        if let Some(i) = at {
            let d = &mut self.defs[i];
            d.loc = loc;
            d.val = val;
        } else {
            if !self.def_ix.is_empty() || self.defs.len() == SCAN {
                if self.def_ix.is_empty() {
                    self.def_ix = self.defs.iter().enumerate().map(|(i, d)| (key(d), i)).collect();
                }
                self.def_ix.insert(k, self.defs.len());
            }
            self.defs.push(Def { f, loc, val });
        }
    }

    /// Declares `f` a loop-back field from a run (a file the job writes
    /// and reads back, found as it runs): in effect when the evaluation is
    /// next quiet, within the same evaluation; readers that bound `f`
    /// otherwise run again and read its round value (`init` until a run
    /// publishes one). As `Engine::declare_loopback` between evaluations.
    pub fn declare_loopback(&mut self, f: Field, init: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f, &init))
        } else {
            serde_json::Value::Null
        };
        if !self.sh.loops.contains_key(&f) {
            super::lock(&self.sh.pending_loops).push((f, init));
        }
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "declare_loopback", a, serde_json::Value::Null);
    }

    /// Publishes a loop-back field's next round value.
    pub fn publish(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f, &x))
        } else {
            serde_json::Value::Null
        };
        let r = self.publish_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "publish", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn publish_inner(&mut self, f: Field, x: ValueId) {
        if let Some(p) = self.pubs.iter_mut().find(|p| p.0 == f) {
            p.1 = x;
        } else {
            self.pubs.push((f, x));
        }
    }

    fn spec_id(&mut self, tag: u64, op: OpId, args: &[ValueId]) -> u64 {
        let mut w: Vec<u64> = Vec::with_capacity(args.len() + 3);
        w.push(self.id);
        w.push(tag);
        w.push(u64::from(op.0));
        w.extend(args.iter().map(|a| a.raw()));
        let h = hash_words(0x6b1d, &w);
        let n = self.dup.entry(h).or_insert(0);
        *n += 1;
        hash_words(h, &[u64::from(*n)])
    }

    /// Unfolds a child: a node placed after this one's earlier children.
    pub fn child(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&op, &args))
        } else {
            serde_json::Value::Null
        };
        let r = self.child_inner(op, args);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "child", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn child_inner(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        let id = self.spec_id(1, op, &args);
        let i = self.kids.iter().filter(|k| matches!(k, Spec::Node { .. })).count();
        self.kids.push(Spec::Node {
            id,
            op,
            args: Arc::from(args),
        });
        i
    }

    /// A continuation: a node placed after this one's subtree, at its level.
    pub fn follow(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&op, &args))
        } else {
            serde_json::Value::Null
        };
        let r = self.follow_inner(op, args);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "follow", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn follow_inner(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        let id = self.spec_id(2, op, &args);
        let i = self.conts.len();
        self.conts.push(Spec::Node {
            id,
            op,
            args: Arc::from(args),
        });
        i
    }

    /// Opens a scope among the children (closed by `close_scope`).
    pub fn open_scope(&mut self) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&())
        } else {
            serde_json::Value::Null
        };
        let r = self.open_scope_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "open_scope", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn open_scope_inner(&mut self) {
        let id = self.spec_id(3, OpId(0), &[]);
        self.kids.push(Spec::Marker { id, open: true });
    }

    pub fn close_scope(&mut self) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&())
        } else {
            serde_json::Value::Null
        };
        let r = self.close_scope_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "close_scope", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn close_scope_inner(&mut self) {
        let id = self.spec_id(4, OpId(0), &[]);
        self.kids.push(Spec::Marker { id, open: false });
    }

    /// The output of child `i` of this run.
    ///
    /// # Errors
    ///
    /// [`Stop`] if the child has not produced it yet.
    pub fn output_of(&mut self, i: usize) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&i,))
        } else {
            serde_json::Value::Null
        };
        let r = self.output_of_inner(i);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "output_of", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn output_of_inner(&mut self, i: usize) -> Result<ValueId, Stop> {
        let Some(id) = self
            .kids
            .iter()
            .filter_map(|k| match k {
                Spec::Node { id, .. } => Some(*id),
                Spec::Marker { .. } => None,
            })
            .nth(i)
        else {
            return Ok(ValueId::UNIT);
        };
        self.output_of_node(NodeId(id))
    }

    fn output_of_node(&mut self, n: NodeId) -> Result<ValueId, Stop> {
        let slot = self.sh.slot_of(n.0).filter(|&s| self.sh.rec(s).st() != S_FREE);
        let made_now = self.kids.iter().chain(&self.conts).any(|s| s.id() == n.0);
        let Some(s) = slot else {
            if made_now {
                return Err(self.stop(Wait {
                    on: Subj::Child(n.0),
                    field: None,
                    prefix: None,
                    kind: "output",
                    has_prev: false,
                }));
            }
            return Ok(ValueId::UNIT);
        };
        if made_now {
            // A child this run already made exists only if the committed
            // unfolding has it: otherwise the partial commit makes it.
            let committed = {
                let d = self.sh.rec(self.me).data();
                d.kids.contains(&s) || d.conts.contains(&s)
            };
            if !committed {
                return Err(self.stop(Wait {
                    on: Subj::Child(n.0),
                    field: None,
                    prefix: None,
                    kind: "output",
                    has_prev: false,
                }));
            }
        }
        let gn = self.sh.rec(s).gn.load(Ordering::Acquire);
        match self.sh.q_out(s, true, self.me) {
            Q::Wait(w) => Err(self.stop(w)),
            Q::Ans(val) => {
                self.log.push(Pred::Out {
                    node: s,
                    gn,
                    val,
                    spec: false,
                });
                Ok(val.unwrap_or(ValueId::UNIT))
            }
        }
    }

    /// Applies `op` to `args`: a memoized application in a unit of its own.
    ///
    /// # Errors
    ///
    /// `Stop` until the application has its result.
    pub fn apply(&mut self, op: OpId, args: Vec<ValueId>) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&op, &args))
        } else {
            serde_json::Value::Null
        };
        let r = self.apply_inner(op, args);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "apply", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn apply_inner(&mut self, op: OpId, args: Vec<ValueId>) -> Result<ValueId, Stop> {
        let a = self.sh.app_node(op, args);
        let gn = self.sh.rec(a).gn.load(Ordering::Acquire);
        match self.sh.q_out(a, true, self.me) {
            Q::Wait(mut w) => {
                w.kind = "application";
                Err(self.stop(w))
            }
            Q::Ans(val) => {
                let val = val.unwrap_or(ValueId::UNIT);
                self.log.push(Pred::App { node: a, gn, val });
                Ok(val)
            }
        }
    }

    /// Adds `x` to accumulator `f` at this node.
    pub fn contribute(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f, &x))
        } else {
            serde_json::Value::Null
        };
        let r = self.contribute_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "contribute", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn contribute_inner(&mut self, f: Field, x: ValueId) {
        if let Some(c) = self.contribs.iter_mut().find(|c| c.0 == f) {
            if let Some(m) = self.sh.accs.get(&f) {
                c.1 = m.combine(&self.sh.values, c.1, x);
            } else {
                c.1 = x;
            }
        } else {
            self.contribs.push((f, x));
        }
    }

    /// The fold of `f`'s contributions after `from` (`None`: the unit's
    /// start) and before this node.
    ///
    /// # Errors
    ///
    /// `Stop` while a node in the range is in work.
    pub fn acc_range(&mut self, f: Field, from: Option<NodeId>) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f, &from))
        } else {
            serde_json::Value::Null
        };
        let r = self.acc_range_inner(f, from);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_range", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn acc_range_inner(&mut self, f: Field, from: Option<NodeId>) -> Result<ValueId, Stop> {
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
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f, &from, &to))
        } else {
            serde_json::Value::Null
        };
        let r = self.acc_range_to_inner(f, from, to);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_range_to", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn acc_range_to_inner(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<ValueId, Stop> {
        let from = from.map(|n| n.0);
        let to = to.map(|n| n.0);
        let q = {
            let (sh, me) = (self.sh, self.me);
            sh.sread(self.unit, || sh.q_range(me, f, from, to, true, me))
        };
        match q {
            Q::Wait(w) => {
                self.ask = Some(super::Ask::Range(f, from, to));
                Err(self.stop(w))
            }
            Q::Ans((val, last)) => {
                self.log.push(Pred::Range {
                    f,
                    at: self.me,
                    from,
                    to,
                    val,
                    last,
                });
                Ok(val)
            }
        }
    }

    /// The kind of the scope this node is in.
    ///
    /// # Errors
    ///
    /// `Stop` while the scope structure before it is undecided.
    pub fn scope_kind(&mut self) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&())
        } else {
            serde_json::Value::Null
        };
        let r = self.scope_kind_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "scope_kind", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn scope_kind_inner(&mut self) -> Result<Option<ValueId>, Stop> {
        let q = {
            let (sh, me) = (self.sh, self.me);
            sh.sread(self.unit, || sh.q_kind(me, true, me))
        };
        match q {
            Q::Wait(w) => Err(self.stop(w)),
            Q::Ans((val, opener)) => {
                self.log.push(Pred::Kind {
                    at: self.me,
                    val,
                    opener,
                });
                Ok(val)
            }
        }
    }

    /// Sets the kind of the scopes this node opens (a kind-pending effect).
    pub fn set_kind(&mut self, k: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&k,))
        } else {
            serde_json::Value::Null
        };
        let r = self.set_kind_inner(k);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "set_kind", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn set_kind_inner(&mut self, k: ValueId) {
        self.kind = Some(k);
    }

    /// Sets the kinds of the frames this node opens, outermost first (a
    /// kind-pending effect opening several); frames past the list take the
    /// kind of `set_kind`.
    pub fn set_kinds(&mut self, ks: &[ValueId]) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&ks,))
        } else {
            serde_json::Value::Null
        };
        self.kinds = ks.to_vec();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "set_kinds", a, serde_json::Value::Null);
    }

    /// Decides a pending effect.
    pub fn set_effect(&mut self, e: Effect) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&e,))
        } else {
            serde_json::Value::Null
        };
        let r = self.set_effect_inner(e);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "set_effect", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn set_effect_inner(&mut self, e: Effect) {
        self.effect = Some(e);
    }

    /// Whether this node's effect is decided by its run.
    #[must_use]
    pub fn effect_pending(&self) -> bool {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&())
        } else {
            serde_json::Value::Null
        };
        let r = self.effect_pending_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "effect_pending", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn effect_pending_inner(&self) -> bool {
        self.peff
    }

    /// Decides a pending claim: the first `n` following siblings.
    /// Narrows this node's definition set to `m`, a subset of what its
    /// text declares ([`crate::Operation::may_define`]), from what the run
    /// has read so far: readers waiting on the node for a field outside
    /// `m` stop waiting and bind past it. Applied with the run's result (a
    /// stop included); a change that re-runs the node restores the text's
    /// answer until it narrows again. Defining outside `m` refutes the
    /// declaration, as defining outside the text's answer does.
    pub fn narrow(&mut self, m: crate::api::MayDefine) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&m,))
        } else {
            serde_json::Value::Null
        };
        let r = self.narrow_inner(m);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "narrow", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn narrow_inner(&mut self, m: crate::api::MayDefine) {
        self.narrowed = Some(m);
    }

    pub fn claim(&mut self, n: usize) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&n,))
        } else {
            serde_json::Value::Null
        };
        let r = self.claim_inner(n);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "claim", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn claim_inner(&mut self, n: usize) {
        self.claim = Some(n);
    }

    /// The claimed siblings' texts.
    pub fn claimed(&self) -> Vec<(OpId, Vec<ValueId>)> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&())
        } else {
            serde_json::Value::Null
        };
        let r = self.claimed_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "claimed", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn claimed_inner(&self) -> Vec<(OpId, Vec<ValueId>)> {
        if let Some(list) = self.static_claimed.clone() {
            let mut out = Vec::new();
            let mut rec = Vec::new();
            for s in list {
                let (op, a) = self.sh.text(s);
                rec.push((s, self.sh.rec(s).gn.load(Ordering::Acquire), self.sh.text_hash(s)));
                out.push((op, a.to_vec()));
            }
            self.side.borrow_mut().push(Pred::Claimed { list: rec });
            return out;
        }
        let n = self.claim.unwrap_or(0);
        (0..n).filter_map(|i| self.following(i)).collect()
    }

    /// The `i`-th following sibling's text.
    pub fn following(&self, i: usize) -> Option<(OpId, Vec<ValueId>)> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&i,))
        } else {
            serde_json::Value::Null
        };
        let r = self.following_inner(i);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "following", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn following_inner(&self, i: usize) -> Option<(OpId, Vec<ValueId>)> {
        // Asked in order (0, 1, 2, ...), the walk resumes where the last
        // answer was; anything else walks from the start. (A changed
        // structure is caught when the log is checked.)
        let cur = self
            .sib_cur
            .get()
            .filter(|&(j, x, gn)| j < i && self.sh.rec(x).gn.load(std::sync::atomic::Ordering::Acquire) == gn);
        let got = {
            let (sh, me) = (self.sh, self.me);
            let (got, seen) = sh.sread(self.unit, || match cur {
                Some((j, x, _)) => sh.q_sib_from(me, x, j + 1, i),
                None => sh.q_sib(me, i),
            });
            // The siblings passed over (held by other claimers): a release
            // of one changes this answer, so it notifies this reader.
            for x in seen {
                if got.is_none_or(|g| g.0 != x) {
                    sh.sib_subscribe(x, me, u32::MAX);
                }
            }
            got
        };
        if let Some((x, gn, _)) = got {
            self.sib_cur.set(Some((i, x, gn)));
        }
        let mut logged = self.sib_logged.borrow_mut();
        if logged.is_none() {
            let mut set = crate::values::FxHashSet::default();
            for p in self.log.iter().chain(self.side.borrow().iter()) {
                if let Pred::Sib { i: j, .. } = p {
                    set.insert(*j);
                }
            }
            *logged = Some(set);
        }
        #[allow(clippy::cast_possible_truncation)]
        if logged.as_mut().is_some_and(|set| set.insert(i as u32)) {
            #[allow(clippy::cast_possible_truncation)]
            self.side.borrow_mut().push(Pred::Sib { i: i as u32, ans: got });
        }
        got.map(|(s, _, _)| {
            let (op, a) = self.sh.text(s);
            (op, a.to_vec())
        })
    }

    /// The outputs of the stream this node claims: the following leaves up
    /// to the one `until` ends it before.
    ///
    /// # Errors
    ///
    /// `Stop` while an element is in work.
    pub fn claim_stream(&mut self) -> Result<Vec<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&())
        } else {
            serde_json::Value::Null
        };
        let r = self.claim_stream_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "claim_stream", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn claim_stream_inner(&mut self) -> Result<Vec<ValueId>, Stop> {
        let q = {
            let (sh, me) = (self.sh, self.me);
            sh.sread(self.unit, || sh.q_stream(me, true, me))
        };
        match q {
            Q::Wait(w) if w.kind == "feeds itself" => {
                let mut waiters = vec![crate::api::Waiter {
                    node: NodeId(self.id),
                    on: crate::api::WaitOn::Queued,
                }];
                if let Subj::Node(x) = w.on {
                    waiters.push(crate::api::Waiter {
                        node: self.sh.node_id(x),
                        on: crate::api::WaitOn::Node(NodeId(self.id), None),
                    });
                }
                let rounds = rlock(&self.sh.limits).idle_rounds + 1;
                self.sh.fail(crate::api::EvalError::NoProgress {
                    rounds,
                    waiters,
                    report: Box::default(),
                });
                Err(self.stop(w))
            }
            Q::Wait(w) => Err(self.stop(w)),
            Q::Ans((outs, seen, reads)) => {
                self.log.push(Pred::Stream {
                    seen,
                    outs: outs.clone(),
                    reads,
                });
                Ok(outs)
            }
        }
    }

    /// The nearest previous sibling whose text passes `pred`.
    pub fn prev_sibling_where(&self, pred: &dyn Fn(OpId, &[ValueId]) -> bool) -> Option<NodeId> {
        #[cfg(feature = "observe")]
        {
            let asked = std::cell::RefCell::new(Vec::new());
            let r = self.prev_sibling_where_inner(&|op, args| {
                let b = pred(op, args);
                asked.borrow_mut().push((op, args.to_vec(), b));
                b
            });
            crate::record::call(
                &self.calls,
                "prev_sibling_where",
                crate::record::j(&asked.into_inner()),
                crate::record::j(&r),
            );
            r
        }
        #[cfg(not(feature = "observe"))]
        self.prev_sibling_where_inner(pred)
    }

    #[inline]
    fn prev_sibling_where_inner(&self, pred: &dyn Fn(OpId, &[ValueId]) -> bool) -> Option<NodeId> {
        let mut seen = Vec::new();
        let mut ans = None;
        let sh = self.sh;
        let me = self.me;
        sh.sread(self.unit, || {
            seen.clear();
            ans = None;
            sh.preceding(me, |x, skipped| {
                seen.push((
                    x,
                    sh.rec(x).gn.load(Ordering::Acquire),
                    sh.text_hash(x) ^ u64::from(skipped),
                ));
                if skipped {
                    return true;
                }
                let (op, a) = sh.text(x);
                if pred(op, &a) {
                    ans = Some(x);
                    return false;
                }
                true
            });
        });
        self.side.borrow_mut().push(Pred::PrevSib { seen, ans });
        ans.map(|s| self.sh.node_id(s))
    }

    /// Pushes `x` on sequence `f` in this node's scope.
    pub fn push_contribution(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f, &x))
        } else {
            serde_json::Value::Null
        };
        let r = self.push_contribution_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "push_contribution", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn push_contribution_inner(&mut self, f: Field, x: ValueId) {
        self.set_def(f, Locality::Local, DefVal::Push(x));
    }

    /// The last element of sequence `f` in this node's scope.
    ///
    /// # Errors
    ///
    /// `Stop` while an earlier node of the scope is in work.
    pub fn prev_contribution(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f,))
        } else {
            serde_json::Value::Null
        };
        let r = self.prev_contribution_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "prev_contribution", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn prev_contribution_inner(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        self.seq_read(f, DMode::Prev)
    }

    /// Sequence `f` in this node's scope, before it.
    ///
    /// # Errors
    ///
    /// `Stop` while an earlier node of the scope is in work.
    pub fn contributions_before_me(&mut self, f: Field) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f,))
        } else {
            serde_json::Value::Null
        };
        let r = self.contributions_before_me_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "contributions_before_me", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn contributions_before_me_inner(&mut self, f: Field) -> Result<ValueId, Stop> {
        Ok(self.seq_read(f, DMode::Before)?.unwrap_or(ValueId::NIL))
    }

    /// Takes the last element of sequence `f` (replacing it by `r`).
    ///
    /// # Errors
    ///
    /// `Stop` while an earlier node of the scope is in work.
    pub fn take_prev_contribution(&mut self, f: Field, r: Option<ValueId>) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f, &r))
        } else {
            serde_json::Value::Null
        };
        let r = self.take_prev_contribution_inner(f, r);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "take_prev_contribution", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn take_prev_contribution_inner(&mut self, f: Field, r: Option<ValueId>) -> Result<Option<ValueId>, Stop> {
        let got = self.seq_read(f, DMode::Prev)?;
        let val = match r {
            Some(x) => DefVal::Repl(x),
            None => DefVal::Take,
        };
        self.set_def(f, Locality::Local, val);
        Ok(got)
    }

    /// Sets an environment field of the unit this application owns.
    pub fn set_env(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f, &x))
        } else {
            serde_json::Value::Null
        };
        let r = self.set_env_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "set_env", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn set_env_inner(&mut self, f: Field, x: ValueId) {
        if let Some(e) = self.env.iter_mut().find(|e| e.0 == f) {
            e.1 = x;
        } else {
            self.env.push((f, x));
        }
    }

    /// Keeps `token` if this run stops: the next run gets it from
    /// `parked` when its reads so far still hold.
    pub fn park(&mut self, token: Box<dyn Any + Send>) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&())
        } else {
            serde_json::Value::Null
        };
        let r = self.park_inner(token);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "park", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn park_inner(&mut self, token: Box<dyn Any + Send>) {
        self.token = Some(token);
    }

    /// The token a stopped run of this node kept.
    pub fn parked(&mut self) -> Option<Box<dyn Any + Send>> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&())
        } else {
            serde_json::Value::Null
        };
        let r = self.parked_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "parked", a, crate::record::j(&r.is_some()));
        r
    }

    #[inline]
    fn parked_inner(&mut self) -> Option<Box<dyn Any + Send>> {
        self.kept.take()
    }

    /// Drops the kept state: the run starts over.
    pub fn restart(&mut self) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&())
        } else {
            serde_json::Value::Null
        };
        let r = self.restart_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "restart", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn restart_inner(&mut self) {
        self.kept = None;
        self.restart = true;
        self.log.clear();
        // Starting over drops what the first pass made: its definitions,
        // children, follows and the rest are not this run's output.
        self.side.borrow_mut().clear();
        self.defs.clear();
        self.def_ix.clear();
        self.contribs.clear();
        self.pubs.clear();
        self.kids.clear();
        self.conts.clear();
        self.effect = None;
        self.kind = None;
        self.kinds.clear();
        self.claim = None;
        self.narrowed = None;
        self.diag.clear();
        self.env.clear();
        self.dup.clear();
        self.sib_cur.set(None);
        *self.sib_logged.borrow_mut() = None;
    }

    pub fn diagnostic(&mut self, msg: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&msg,))
        } else {
            serde_json::Value::Null
        };
        let r = self.diagnostic_inner(msg);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "diagnostic", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn diagnostic_inner(&mut self, msg: ValueId) {
        self.diag.push(msg);
    }

    /// A pure text computation, memoized by the value store.
    pub fn memo_text(&self, a: ValueId, b: ValueId, produce: impl FnOnce(&Values) -> ValueId) -> ValueId {
        let key = (a, b);
        if let Some(&v) = rlock(&self.sh.text_memo).get(&key) {
            return v;
        }
        let v = produce(&self.sh.values);
        super::wlock(&self.sh.text_memo).insert(key, v);
        v
    }

    /// Merges the reads recorded through `&self` into the log.
    pub(crate) fn flush_side(&mut self) {
        let side = std::mem::take(self.side.get_mut());
        self.log.extend(side);
    }

    /// Whether this run is speculative.
    #[must_use]
    pub fn is_speculative(&self) -> bool {
        Shared::spec_run()
    }

    /// The values' epoch ([`Engine::values_epoch`]): every [`ValueId`] this
    /// run makes or reads stays valid while it is unchanged, so a cache of
    /// interned values keyed by `(epoch, content)` needs no pinning.
    #[must_use]
    pub fn values_epoch(&self) -> u64 {
        0
    }

    /// Whether `f` is a loop-back field.
    #[must_use]
    pub fn is_loopback(&mut self, f: Field) -> bool {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() {
            crate::record::j(&(&f,))
        } else {
            serde_json::Value::Null
        };
        let r = self.is_loopback_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "is_loopback", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn is_loopback_inner(&mut self, f: Field) -> bool {
        let val = self.sh.loops.contains_key(&f);
        self.log.push(Pred::IsLoop { f, val });
        val
    }

    /// Accounts `n` units of work.
    ///
    /// # Errors
    ///
    /// Never in this core.
    pub fn tick(&mut self, _n: u64) -> Result<(), Stop> {
        Ok(())
    }
}

impl Shared {
    /// The app node of an application, made on first use.
    pub(crate) fn app_node(&self, op: OpId, args: Vec<ValueId>) -> Slot {
        let mut w: Vec<u64> = args.iter().map(|a| a.raw()).collect();
        w.push(u64::from(op.0));
        let key = hash_words(0xa99, &w);
        let mut apps = lock(&self.apps);
        if let Some(&s) = apps.get(&key)
            && self.rec(s).st() != S_FREE
        {
            return s;
        }
        let u = self.new_unit_rec(NIL);
        let mut data = super::Data::new(super::Kind::Node, super::Role::App, op, Arc::from(args), NIL);
        data.ex().app_unit = u;
        data.ex().app_key = key;
        let s = self.alloc(u, key, data, 0, NIL);
        self.unit(u).app.store(s, Ordering::Release);
        self.rec(s).set(super::F_APP | super::F_NEEDS_RUN);
        apps.insert(key, s);
        drop(apps);
        let mut notes = super::order::Notes::default();
        {
            let _g = self.wunit(u);
            self.link_after(u, NIL, &[s]);
            self.register(s, &mut notes);
            notes.frames_from.push(s);
            self.settle_structure(u, &mut notes);
        }
        self.dispatch(&mut notes, NIL);
        self.enqueue(s);
        s
    }
}
