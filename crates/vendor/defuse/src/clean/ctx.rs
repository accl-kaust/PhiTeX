//! A run's view of a world: every read is asked of one immutable world and
//! recorded as a predicate; every write goes into the run's result.

use std::any::Any;
use std::sync::Arc;

use super::core::Core;
use super::world::{Def, Id, NONE, Place, Pos, Read, St, World};
use crate::api::{Effect, FieldAt, Locality, NodeId, OpId, Stop};
use crate::values::FxHashMap;
use crate::values::{Field, ValueId, Values};

/// A placement a run made, before the merge gives it an id.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Spec {
    pub op: OpId,
    pub args: Arc<[ValueId]>,
    pub place: Place,
}

/// Why a run stopped: what it waits on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Wait {
    /// A node in work: its result (a definition, an output, a structure).
    Node(Id, Option<Field>),
    /// A child this run places, not yet made: the run's unfolding is
    /// committed and the run waits for the child.
    Child(usize),
    /// An environment field neither supplied nor declared absent.
    Env(Field),
}

/// A run's view of the store.
pub struct Ctx<'a> {
    pub(crate) core: &'a Core,
    pub(crate) w: &'a World,
    pub(crate) me: Id,
    pub(crate) unit: u32,
    pub(crate) op: OpId,
    pub(crate) args: Arc<[ValueId]>,
    pub(crate) reads: Vec<Read>,
    pub(crate) defs: Vec<Def>,
    pub(crate) contribs: Vec<(Field, ValueId)>,
    pub(crate) pubs: Vec<(Field, ValueId)>,
    pub(crate) kids: Vec<Spec>,
    pub(crate) conts: Vec<Spec>,
    pub(crate) effect: Option<Effect>,
    pub(crate) kinds: Vec<ValueId>,
    pub(crate) diag: Vec<ValueId>,
    pub(crate) wait: Option<Wait>,
    pub(crate) kept: Option<Box<dyn Any + Send>>,
    pub(crate) restart: bool,
    pub(crate) error: Option<crate::api::EvalError>,
    /// The run's claim (`claim`).
    pub(crate) claim: Option<usize>,
    /// Applications this run asked for that are not made yet.
    pub(crate) apps: Vec<(Id, OpId, Arc<[ValueId]>)>,
    /// Environment fields set by an application's run (`set_env`).
    pub(crate) env: Vec<(Field, ValueId)>,
    /// Where each definition, publication, contribution and environment
    /// field is in its list; the children unfolded (markers aside).
    ix: FxHashMap<(u8, Field, u8), usize>,
    nkids: usize,
    /// The run stopped on a pending definition set.
    pub(crate) pset_wait: bool,
    /// Reads of a definer still to run may take its predicted value.
    pub(crate) predict: bool,
    /// Committed values only: a read of a definer in work waits for it.
    pub(crate) careful: bool,
    /// It read a definer in work (a guess the merge checks).
    pub(crate) guessed: bool,
    /// Reads answered by a prediction.
    pub(crate) predicted: u32,
    /// A speculative run's read budget; spent, the run stops (`fuel_out`).
    pub(crate) fuel: Option<u64>,
    pub(crate) fuel_out: bool,
    /// The range a stopped run waits on (field, from, to).
    pub(crate) range_wait: Option<(Field, Id, Id, bool)>,
    /// The bound the run narrowed its pending set to.
    pub(crate) narrowed: Option<super::world::May>,
    /// The calls of a recorded run (`observe`; `RecOp::run` collects them).
    #[cfg(feature = "observe")]
    pub(crate) calls: std::cell::RefCell<Option<crate::record::Calls>>,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(core: &'a Core, w: &'a World, me: Id, kept: Option<Box<dyn Any + Send>>) -> Ctx<'a> {
        let n = w.node(me).expect("a live node");
        Ctx {
            core,
            w,
            me,
            unit: n.unit,
            op: n.text.op,
            args: n.text.args.clone(),
            reads: Vec::new(),
            defs: Vec::new(),
            contribs: Vec::new(),
            pubs: Vec::new(),
            kids: Vec::new(),
            conts: Vec::new(),
            effect: None,
            kinds: Vec::new(),
            diag: Vec::new(),
            wait: None,
            kept,
            restart: false,
            error: None,
            claim: None,
            apps: Vec::new(),
            env: Vec::new(),
            ix: FxHashMap::default(),
            nkids: 0,
            pset_wait: false,
            predict: false,
            careful: false,
            guessed: false,
            predicted: 0,
            fuel: None,
            fuel_out: false,
            range_wait: None,
            narrowed: None,
            #[cfg(feature = "observe")]
            calls: std::cell::RefCell::new(None),
        }
    }

    /// Spends `n` of a speculative run's fuel; spent out, the run stops.
    fn spend(&mut self, n: u64) -> Result<(), Stop> {
        if let Some(fuel) = &mut self.fuel {
            if *fuel < n {
                *fuel = 0;
                self.fuel_out = true;
                return Err(self.stop(Wait::Node(self.me, None)));
            }
            *fuel -= n;
        }
        Ok(())
    }

    #[track_caller]
    fn stop(&mut self, w: Wait) -> Stop {
        if super::core::debug() {
            // (Where it stopped, and the subject as this run's world has it.)
            let x = match w {
                Wait::Node(x, _) => Some(x),
                _ => None,
            };
            eprintln!(
                "stopped {} at ctx.rs:{} on {w:?}: {:?} (op {:?}, reads hinted {:?})",
                self.me,
                std::panic::Location::caller().line(),
                x.and_then(|x| self.w.node(x)).map(|n| (n.st, n.pos)),
                self.w.node(self.me).map(|n| self.core.op_name(n.text.op)),
                self.w.node(self.me).map(|n| n.text.stat.reads.is_some()),
            );
        }
        self.wait = Some(w);
        Stop { _private: () }
    }

    #[must_use]
    pub fn values(&self) -> &'a Values {
        &self.core.values
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
        NodeId(self.me)
    }

    /// The value of `f` reaching this node: the nearest earlier visible
    /// definition, a loop-back field's round value, or the environment.
    ///
    /// # Errors
    ///
    /// `Stop` when the answer's subject is still in work.
    pub fn use_field(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, )) } else { serde_json::Value::Null };
        let r = self.use_field_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_field", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_field_inner(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        Ok(self.read_at(f, self.me, false, 0)?.map(|x| x.0))
    }

    /// Reads `f` at node `at` (before it, or after its subtree when
    /// `after`): the value and the site.
    fn read_at(&mut self, f: Field, at: Id, after: bool, proj: u16) -> Result<Option<(ValueId, Id)>, Stop> {
        let t = super::core::prof::start();
        let r = self.read_at_inner(f, at, after, proj);
        super::core::prof::stop(18, t);
        r
    }

    fn read_at_inner(&mut self, f: Field, at: Id, after: bool, proj: u16) -> Result<Option<(ValueId, Id)>, Stop> {
        // (Fuel: one, and one per frame the read walks.)
        let depth = self.w.node(at).map_or(1, |n| self.w.unit(n.unit).frames.get(&n.frame).map_or(1, |fr| u64::from(fr.depth) + 1));
        self.spend(depth)?;
        if let Some(&lv) = self.w.loops.get(&f) {
            self.reads.push(Read::Loop { f, val: lv });
            return Ok(Some((lv, NONE)));
        }
        let Some((u, pos, fr)) = self.w.frame_at(at, after) else {
            return Ok(None);
        };
        let reach = super::core::prof::time(21, || self.w.reaching(u, f, pos, fr, if after { NONE } else { at }));
        // A pending definition set between the site and the reader: wait
        // (speculating, the read guesses past it; a definition it makes
        // re-asks the reader).
        let from = reach.map_or(0, |r| r.0);
        if !self.predict
            && let Some(x) = super::core::prof::time(19, || self.w.pending_set_between(self.core, u, f, from, pos, self.me))
        {
            if super::core::debug() && self.w.node(x).is_some_and(|n| !matches!(n.st, St::Dirty | St::Held)) {
                eprintln!("read {} of {f:?}: pending set of {x} in state {:?}", self.me, self.w.node(x).map(|n| n.st));
            }
            self.pset_wait = true;
            return Err(self.stop(Wait::Node(x, Some(f))));
        }
        match reach {
            Some((_, e)) => {
                let site = e.node;
                let val = match e.val {
                    Some(v) => Some(v),
                    None if self.predict
                        && let Some(v) = self
                            .w
                            .node(site)
                            .and_then(|n| n.text.stat.preds.as_ref())
                            .and_then(|ps| ps.iter().find(|p| p.0 == f).map(|p| p.1)) =>
                    {
                        // The definer's prediction, as a guess the merge
                        // checks.
                        self.predicted += 1;
                        Some(v)
                    }
                    None => {
                        if super::core::debug() && self.w.node(site).is_some_and(|n| matches!(n.st, St::Inert(_) | St::Done)) {
                            eprintln!("site {site} {:?} frame {:?} pos {:?}; reader frame {fr:?}", self.w.node(site).map(|n| n.st), self.w.node(site).map(|n| n.frame), self.w.node(site).map(|n| n.pos));
                        }
                        if super::core::debug() && self.w.node(site).is_some_and(|n| matches!(n.st, St::Inert(_))) {
                            eprintln!("read {} of {f:?} at {at}: pending entry of inert {site} in frame {fr:?} entry {e:?}", self.me);
                        }
                        // Declared, not yet made: wait for the definer.
                        return Err(self.stop(Wait::Node(site, Some(f))));
                    }
                };
                if self.in_work(site) && site != self.me {
                    // (A definer in work holds its last result: taken as a
                    // guess the merge validates, unless running carefully.)
                    if self.careful {
                        return Err(self.stop(Wait::Node(site, Some(f))));
                    }
                    self.guessed = true;
                }
                self.reads.push(Read::Use {
                    f,
                    at,
                    after,
                    site,
                    val,
                    proj,
                });
                Ok(val.map(|v| (v, site)))
            }
            None => {
                let val = self.env_value(f)?;
                self.reads.push(Read::Use {
                    f,
                    at,
                    after,
                    site: NONE,
                    val,
                    proj,
                });
                Ok(val.map(|v| (v, NONE)))
            }
        }
    }

    /// Whether node `id` is in work in this world.
    fn in_work(&self, id: Id) -> bool {
        self.w.node(id).is_some_and(|n| matches!(n.st, St::Dirty | St::Held))
    }

    /// The environment's value of `f` when no definition reaches: supplied,
    /// declared absent (`None`), or unknown (a field never supplied reads
    /// as absent).
    fn env_value(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        Ok(self.w.env_of(self.core, self.unit, f))
    }

    /// The definition of `f` reaching the position just after node `at`
    /// (what it unfolded or claims included): the value and its defining
    /// node (`None`: the unit's start value). `at` is any other node of
    /// this unit, before or past this one (a page builder reading the
    /// state at the pieces after it): a definition coming in or changing
    /// up to `at` re-asks, as for any read. One past this node may reach
    /// this node's own definition, read as it last committed.
    ///
    /// # Errors
    ///
    /// [`Stop`] while the definition (or structure before it) is in work.
    pub fn use_field_at(&mut self, f: Field, at: Option<NodeId>) -> Result<FieldAt, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &at, )) } else { serde_json::Value::Null };
        let r = self.use_field_at_inner(f, at);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_field_at", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_field_at_inner(&mut self, f: Field, at: Option<NodeId>) -> Result<FieldAt, Stop> {
        self.use_field_pos(f, at, true)
    }

    /// The definition of `f` reaching the position just before node `at`
    /// (before or past this one, as [`Ctx::use_field_at`]).
    ///
    /// # Errors
    ///
    /// [`Stop`] while the definition (or structure before it) is in work.
    pub fn use_field_before(&mut self, f: Field, at: NodeId) -> Result<FieldAt, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &at, )) } else { serde_json::Value::Null };
        let r = self.use_field_before_inner(f, at);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_field_before", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_field_before_inner(&mut self, f: Field, at: NodeId) -> Result<FieldAt, Stop> {
        self.use_field_pos(f, Some(at), false)
    }

    fn use_field_pos(&mut self, f: Field, at: Option<NodeId>, after: bool) -> Result<FieldAt, Stop> {
        let Some(at) = at else {
            // The unit's start: its start value.
            let v = self.env_value(f)?;
            self.reads.push(Read::Env { f, val: v });
            return Ok(v.map_or(FieldAt::Unset, |x| FieldAt::Def(x, None)));
        };
        let a = at.0;
        if self.w.node(a).is_none_or(|n| matches!(n.place, Place::Marker(_))) {
            self.reads.push(Read::At { node: a, there: false });
            return Ok(FieldAt::Gone);
        }
        // (Before or past the reader in its unit, not the reader itself.)
        if a == self.me || self.w.node(a).is_none_or(|n| n.unit != self.unit) {
            self.error = Some(crate::api::EvalError::BadPosition {
                node: self.node(),
                at,
            });
            return Ok(FieldAt::Unset);
        }
        self.reads.push(Read::At { node: a, there: true });
        let r = self.read_at(f, a, after, 0)?;
        Ok(match r {
            None => FieldAt::Unset,
            Some((v, site)) => FieldAt::Def(v, (site != NONE).then_some(NodeId(site))),
        })
    }

    /// Uses field `f` through projection `p`: returns the projected value
    /// (`None` if nothing defines it), and the run depends only on it.
    ///
    /// # Errors
    ///
    /// [`Stop`] as for [`Ctx::use_field`].
    pub fn use_field_proj(&mut self, f: Field, p: crate::api::ProjId) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &p, )) } else { serde_json::Value::Null };
        let r = self.use_field_proj_inner(f, p);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_field_proj", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_field_proj_inner(&mut self, f: Field, p: crate::api::ProjId) -> Result<Option<ValueId>, Stop> {
        let r = self.read_at(f, self.me, false, p.0)?;
        Ok(r.map(|(x, _)| self.core.project(p.0, x)))
    }

    /// The environment's value of `f`; waits while it is neither supplied
    /// nor declared absent.
    ///
    /// # Errors
    ///
    /// `Stop` while the field is unknown.
    pub fn use_env(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, )) } else { serde_json::Value::Null };
        let r = self.use_env_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_env", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_env_inner(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        match self.core.env_get(self.w, f) {
            Some(val) => {
                self.reads.push(Read::Env { f, val });
                Ok(val)
            }
            None => Err(self.stop(Wait::Env(f))),
        }
    }

    pub fn define(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &x, )) } else { serde_json::Value::Null };
        let r = self.define_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "define", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn define_inner(&mut self, f: Field, x: ValueId) {
        let loc = self
            .w
            .node(self.me)
            .and_then(|n| n.text.stat.defs.as_ref().and_then(|d| d.iter().find(|e| e.0 == f).map(|e| e.1)))
            .unwrap_or(Locality::Local);
        self.define_with_inner(f, x, loc);
    }

    pub fn define_with(&mut self, f: Field, x: ValueId, loc: Locality) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &x, &loc, )) } else { serde_json::Value::Null };
        let r = self.define_with_inner(f, x, loc);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "define_with", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn define_with_inner(&mut self, f: Field, x: ValueId, loc: Locality) {
        let k = loc.entry_key();
        if let Some(&i) = self.ix.get(&(0, f, k)) {
            let d = &mut self.defs[i];
            d.loc = loc;
            d.val = x;
        } else {
            self.ix.insert((0, f, k), self.defs.len());
            self.defs.push(Def { f, loc, val: x });
        }
    }

    /// Defines `f` as a local of the scope this node opens after itself.
    pub fn define_in_scope(&mut self, f: Field, x: ValueId) {
        self.define_with(f, x, Locality::Scope);
    }

    /// Defines `f` as a local of the `k`-th frame this node opens.
    pub fn define_in_scope_at(&mut self, f: Field, k: u8, x: ValueId) {
        self.define_with(f, x, Locality::ScopeAt(k));
    }

    /// Declares `f` a loop-back field from a run.
    pub fn declare_loopback(&mut self, f: Field, init: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &init, )) } else { serde_json::Value::Null };
        let r = self.declare_loopback_inner(f, init);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "declare_loopback", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn declare_loopback_inner(&mut self, f: Field, init: ValueId) {
        self.core.declare_loop_later(f, init);
    }

    /// Publishes a loop-back field's next round value.
    pub fn publish(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &x, )) } else { serde_json::Value::Null };
        let r = self.publish_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "publish", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn publish_inner(&mut self, f: Field, x: ValueId) {
        if let Some(&i) = self.ix.get(&(1, f, 0)) {
            self.pubs[i].1 = x;
        } else {
            self.ix.insert((1, f, 0), self.pubs.len());
            self.pubs.push((f, x));
        }
    }

    /// Unfolds a child: a node placed after this one's earlier children.
    pub fn child(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&op, &args, )) } else { serde_json::Value::Null };
        let r = self.child_inner(op, args);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "child", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn child_inner(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        let i = self.nkids;
        self.nkids += 1;
        self.kids.push(Spec {
            op,
            args: Arc::from(args),
            place: Place::Child,
        });
        i
    }

    /// A continuation: a node placed after this one's subtree, at its level.
    pub fn follow(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&op, &args, )) } else { serde_json::Value::Null };
        let r = self.follow_inner(op, args);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "follow", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn follow_inner(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        let i = self.conts.len();
        self.conts.push(Spec {
            op,
            args: Arc::from(args),
            place: Place::Cont,
        });
        i
    }

    /// A continuation placed right after an earlier node `at` of this
    /// unit, at `at`'s level. An `at` that is gone, in another unit or not
    /// before this node places it after this one (as `follow`).
    pub fn follow_after(&mut self, at: NodeId, op: OpId, args: Vec<ValueId>) -> usize {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&at, &op, &args, )) } else { serde_json::Value::Null };
        let r = self.follow_after_inner(at, op, args, false);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "follow_after", a, crate::record::j(&r));
        r
    }

    /// As [`Ctx::follow_after`], with `at` before or past this node: a
    /// continuation placed right after a later node of this unit (as a
    /// page builder places a page's finisher after the piece it broke
    /// at). Readers after `at` are re-asked as for any node coming in. An
    /// `at` that is gone, in another unit or this node places it after
    /// this one (as `follow`).
    pub fn follow_after_any(&mut self, at: NodeId, op: OpId, args: Vec<ValueId>) -> usize {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&at, &op, &args, )) } else { serde_json::Value::Null };
        let r = self.follow_after_inner(at, op, args, true);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "follow_after_any", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn follow_after_inner(&mut self, at: NodeId, op: OpId, args: Vec<ValueId>, any: bool) -> usize {
        let ok = self.w.node(at.0).is_some_and(|n| n.unit == self.unit && !matches!(n.place, Place::Marker(_)))
            && if any { at.0 != self.me } else { self.w.before(at.0, self.me) };
        if !ok {
            return self.follow_inner(op, args);
        }
        self.reads.push(Read::At {
            node: at.0,
            there: self.w.node(at.0).is_some(),
        });
        let i = self.conts.len();
        self.conts.push(Spec {
            op,
            args: Arc::from(args),
            place: Place::After(at.0),
        });
        i
    }

    /// Opens a scope among the children (closed by `close_scope`).
    pub fn open_scope(&mut self) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.open_scope_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "open_scope", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn open_scope_inner(&mut self) {
        self.kids.push(Spec {
            op: OpId(0),
            args: Arc::from([]),
            place: Place::Marker(true),
        });
    }

    pub fn close_scope(&mut self) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.close_scope_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "close_scope", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn close_scope_inner(&mut self) {
        self.kids.push(Spec {
            op: OpId(1),
            args: Arc::from([]),
            place: Place::Marker(false),
        });
    }

    /// The output of child `i` of this run.
    ///
    /// # Errors
    ///
    /// [`Stop`] if the child has not produced it yet.
    pub fn output_of(&mut self, i: usize) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&i, )) } else { serde_json::Value::Null };
        let r = self.output_of_inner(i);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "output_of", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn output_of_inner(&mut self, i: usize) -> Result<ValueId, Stop> {
        // The child this run placed `i`-th, if the committed unfolding has
        // it (the same placement order: the same node).
        let Some(k) = self
            .kids
            .iter()
            .enumerate()
            .filter(|(_, s)| !matches!(s.place, Place::Marker(_)))
            .nth(i)
            .map(|(k, _)| k)
        else {
            return Ok(ValueId::UNIT);
        };
        let committed = self
            .w
            .node(self.me)
            .and_then(|n| n.res.as_ref())
            .and_then(|r| r.kids.get(k).copied())
            .filter(|&c| self.w.node(c).is_some_and(|n| n.text.op == self.kids[k].op && n.text.args == self.kids[k].args));
        let Some(c) = committed else {
            return Err(self.stop(Wait::Child(k)));
        };
        self.read_out(c)
    }

    fn read_out(&mut self, c: Id) -> Result<ValueId, Stop> {
        let Some(n) = self.w.node(c) else {
            return Ok(ValueId::UNIT);
        };
        if matches!(n.st, St::Dirty | St::Held) && n.res.is_none() {
            return Err(self.stop(Wait::Node(c, None)));
        }
        let val = n.out();
        self.reads.push(Read::Out { node: c, val });
        Ok(val.unwrap_or(ValueId::UNIT))
    }

    /// Applies `op` to `args`: a memoized application in a unit of its own.
    ///
    /// # Errors
    ///
    /// `Stop` until the application has its result.
    pub fn apply(&mut self, op: OpId, args: Vec<ValueId>) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&op, &args, )) } else { serde_json::Value::Null };
        let r = self.apply_inner(op, args);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "apply", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn apply_inner(&mut self, op: OpId, args: Vec<ValueId>) -> Result<ValueId, Stop> {
        let app = app_id(op, &args);
        match self.w.node(app) {
            None => {
                self.apps.push((app, op, Arc::from(args)));
                Err(self.stop(Wait::Node(app, None)))
            }
            Some(n) if matches!(n.st, St::Dirty | St::Held) => Err(self.stop(Wait::Node(app, None))),
            Some(n) => {
                let val = n.out();
                self.reads.push(Read::Out { node: app, val });
                Ok(val.unwrap_or(ValueId::UNIT))
            }
        }
    }

    /// Adds `x` to accumulator `f` at this node.
    pub fn contribute(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &x, )) } else { serde_json::Value::Null };
        let r = self.contribute_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "contribute", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn contribute_inner(&mut self, f: Field, x: ValueId) {
        if let Some(&i) = self.ix.get(&(2, f, 0)) {
            let c = &mut self.contribs[i];
            c.1 = match self.core.accs.get(&f) {
                Some(m) => m.combine(&self.core.values, c.1, x),
                None => x,
            };
        } else {
            self.ix.insert((2, f, 0), self.contribs.len());
            self.contribs.push((f, x));
        }
    }

    /// The fold of `f`'s contributions after `from` and before this node.
    ///
    /// # Errors
    ///
    /// `Stop` while a node in the range is in work.
    pub fn acc_range(&mut self, f: Field, from: Option<NodeId>) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &from, )) } else { serde_json::Value::Null };
        let r = self.acc_range_inner(f, from);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_range", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn acc_range_inner(&mut self, f: Field, from: Option<NodeId>) -> Result<ValueId, Stop> {
        self.acc_range_to_inner(f, from, None)
    }

    /// The fold of accumulator `f` over the window `(from, to]`.
    ///
    /// # Errors
    ///
    /// [`Stop`] until every node in the window has finished.
    pub fn acc_range_to(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &from, &to, )) } else { serde_json::Value::Null };
        let r = self.acc_range_to_inner(f, from, to);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_range_to", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn acc_range_to_inner(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<ValueId, Stop> {
        self.acc_window(f, from, to, false)
    }

    /// The fold of accumulator `f` over the window `(from, to]`, which may
    /// lie past this node (`to` after it): a reader of the nodes after it,
    /// as a page builder reads the pieces it breaks. This node's own
    /// contribution to `f`, if any, is folded as it last committed.
    ///
    /// # Errors
    ///
    /// [`Stop`] until every node in the window has finished.
    pub fn acc_fold(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &from, &to, )) } else { serde_json::Value::Null };
        let r = self.acc_window(f, from, to, true);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_fold", a, crate::record::stop_or(&r));
        r
    }

    /// The first node after `after` (`None`: the unit's start), before or
    /// past this one, that contributes to accumulator `f`, with its
    /// contribution; `None` when none is left in the unit. A reader that
    /// folds forward a node at a time, as a page builder takes the pieces
    /// after it until it decides, calls it again from the node it got.
    /// What it read is the window up to that node: a contributor that
    /// comes in between, or a change of the node's contribution, re-asks.
    ///
    /// # Errors
    ///
    /// [`Stop`] while a node in the window, up to the next contributor,
    /// is in work and may contribute.
    pub fn acc_next(&mut self, f: Field, after: Option<NodeId>) -> Result<Option<(NodeId, ValueId)>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &after, )) } else { serde_json::Value::Null };
        let r = self.acc_next_inner(f, after);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_next", a, crate::record::stop_or(&r));
        r
    }

    fn acc_next_inner(&mut self, f: Field, after: Option<NodeId>) -> Result<Option<(NodeId, ValueId)>, Stop> {
        let from = after.map_or(NONE, |n| n.0);
        let Some((u, lo, _)) = self.w.window(self.me, from, NONE, true) else {
            return Ok(None);
        };
        // The next committed (or listed) contribution, not this node's.
        let order = &self.w.unit(u).order;
        let next = self.w.contribs.get(&(u, f)).and_then(|m| {
            m.range(std::ops::Bound::Excluded(&lo), std::ops::Bound::Unbounded)
                .filter_map(|(p, _)| order.get(p).map(|&x| (*p, x)))
                .find(|&(_, x)| x != self.me)
        });
        let hi = next.map_or(Pos::MAX, |(p, _)| p + 1);
        // (A node before it in work that may contribute comes first.)
        if let Some(x) = self.w.range_blocker(self.me, f, u, lo, hi) {
            let to = next.map_or(NONE, |(_, x)| x);
            self.range_wait = Some((f, from, to, true));
            return Err(self.stop(Wait::Node(x, Some(f))));
        }
        let val = match self.core.accs.get(&f) {
            Some(mo) => {
                let m = super::world::AccMeasure {
                    m: &**mo,
                    v: &self.core.values,
                };
                self.w.fold(&m, u, f, lo, hi)
            }
            None => ValueId::UNIT,
        };
        let to = next.map_or(NONE, |(_, x)| x);
        self.reads.push(Read::Range { f, from, to, val, past: true });
        Ok(next.map(|(_, x)| (NodeId(x), val)))
    }

    fn acc_window(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>, past: bool) -> Result<ValueId, Stop> {
        let from = from.map_or(NONE, |n| n.0);
        let to = to.map_or(NONE, |n| n.0);
        let Some((u, lo, hi)) = self.w.window(self.me, from, to, past) else {
            return Ok(ValueId::UNIT);
        };
        // A node in the window still in work that may contribute: waited
        // for (one that declares its contributions, or whose last run made
        // none to `f`, is folded as it stands; its change re-asks).
        if let Some(x) = self.w.range_blocker(self.me, f, u, lo, hi) {
            if super::core::debug() && self.w.node(x).is_some_and(|n| !matches!(n.st, St::Dirty | St::Held)) {
                eprintln!("range {} of {f:?}: blocker {x} in state {:?}", self.me, self.w.node(x).map(|n| n.st));
            }
            // (It waits for the whole range: re-parked on the next blocker
            // without running while one is left.)
            self.range_wait = Some((f, from, to, past));
            return Err(self.stop(Wait::Node(x, Some(f))));
        }
        let val = match self.core.accs.get(&f) {
            Some(mo) => {
                let m = super::world::AccMeasure {
                    m: &**mo,
                    v: &self.core.values,
                };
                self.w.fold(&m, u, f, lo, hi)
            }
            None => ValueId::UNIT,
        };
        self.reads.push(Read::Range { f, from, to, val, past });
        Ok(val)
    }

    /// The kind of the scope this node is in.
    ///
    /// # Errors
    ///
    /// `Stop` while the scope structure before it is undecided.
    pub fn scope_kind(&mut self) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.scope_kind_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "scope_kind", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn scope_kind_inner(&mut self) -> Result<Option<ValueId>, Stop> {
        let val = self.w.kind_at(self.me);
        self.reads.push(Read::Kind { at: self.me, val });
        Ok(val)
    }

    /// Sets the kind of the scopes this node opens (a kind-pending effect).
    pub fn set_kind(&mut self, k: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&k, )) } else { serde_json::Value::Null };
        let r = self.set_kind_inner(k);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "set_kind", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn set_kind_inner(&mut self, k: ValueId) {
        self.kinds = vec![k];
    }

    /// Sets the kinds of the frames this node opens, outermost first.
    pub fn set_kinds(&mut self, ks: &[ValueId]) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&ks, )) } else { serde_json::Value::Null };
        let r = self.set_kinds_inner(ks);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "set_kinds", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn set_kinds_inner(&mut self, ks: &[ValueId]) {
        self.kinds = ks.to_vec();
    }

    /// Decides a pending effect.
    pub fn set_effect(&mut self, e: Effect) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&e, )) } else { serde_json::Value::Null };
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
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.effect_pending_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "effect_pending", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn effect_pending_inner(&self) -> bool {
        self.w.node(self.me).is_some_and(|n| n.text.stat.effect.is_none())
    }

    /// Narrows this node's definition set to `m`.
    pub fn narrow(&mut self, m: crate::api::MayDefine) {
        #[cfg(feature = "observe")]
        {
            let a = if self.calls.borrow().is_some() { crate::record::j(&(&m,)) } else { serde_json::Value::Null };
            crate::record::call(&self.calls, "narrow", a, serde_json::Value::Null);
        }
        self.narrowed = Some(m.into());
    }

    /// Claims the first `n` following siblings (applied when the run
    /// commits; it overrides the text's declared claim).
    pub fn claim(&mut self, n: usize) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&n, )) } else { serde_json::Value::Null };
        let r = self.claim_inner(n);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "claim", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn claim_inner(&mut self, n: usize) {
        self.claim = Some(n);
    }

    /// The claimed siblings' texts: a declared count claim's current
    /// siblings, else the first `n` following ones of this run's `claim(n)`.
    #[must_use]
    pub fn claimed(&mut self) -> Vec<(OpId, Vec<ValueId>)> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.claimed_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "claimed", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn claimed_inner(&mut self) -> Vec<(OpId, Vec<ValueId>)> {
        let st = self.w.claims.get(&self.me);
        if let Some(st) = st.filter(|st| matches!(st.decl, crate::api::Claim::Count(_)) && self.claim.is_none()) {
            let list: Vec<(Id, usize)> = st.taken.iter().map(|&x| (x, super::claims::key_of(self.w, x))).collect();
            self.reads.push(Read::Claimed { list: list.clone() });
            return list
                .iter()
                .filter_map(|&(x, _)| self.w.node(x).map(|n| (n.text.op, n.text.args.to_vec())))
                .collect();
        }
        let n = self.claim.unwrap_or(0);
        (0..n).filter_map(|i| self.following_inner(i)).collect()
    }

    /// The `i`-th following sibling's text (never waits).
    #[must_use]
    pub fn following(&mut self, i: usize) -> Option<(OpId, Vec<ValueId>)> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&i, )) } else { serde_json::Value::Null };
        let r = self.following_inner(i);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "following", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn following_inner(&mut self, i: usize) -> Option<(OpId, Vec<ValueId>)> {
        let (w, me) = (self.w, self.me);
        let got = w.forward(me, |_, x| w.taken_from(me, x)).nth(i);
        let ans = got.map(|x| (x, super::claims::key_of(w, x)));
        let rd = Read::Sib {
            i: u32::try_from(i).unwrap_or(u32::MAX),
            ans,
        };
        if !self.reads.contains(&rd) {
            self.reads.push(rd);
        }
        got.and_then(|x| w.node(x)).map(|n| (n.text.op, n.text.args.to_vec()))
    }

    /// The outputs of the stream this node claims: the following leaves up
    /// to the one its `until` hooks end it before.
    ///
    /// # Errors
    ///
    /// `Stop` while an element is in work.
    pub fn claim_stream(&mut self) -> Result<Vec<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.claim_stream_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "claim_stream", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn claim_stream_inner(&mut self) -> Result<Vec<ValueId>, Stop> {
        let got = self.w.stream(self.core, self.me);
        // An element that read a definition of this node's feeds the
        // stream itself: no order makes progress.
        let feeds = |x: Id| {
            self.w.node(x).and_then(|n| n.res.as_ref()).is_some_and(|r| {
                r.reads.iter().any(|rd| matches!(rd, Read::Use { site, .. } if *site == self.me))
            })
        };
        let fed = match &got {
            Ok((outs, _, _)) => outs.iter().map(|o| o.0).find(|&x| feeds(x)),
            Err(x) => Some(*x).filter(|&x| feeds(x)),
        };
        if let Some(x) = fed {
            self.error = Some(crate::api::EvalError::NoProgress {
                rounds: 0,
                waiters: vec![
                    crate::api::Waiter {
                        node: NodeId(self.me),
                        on: crate::api::WaitOn::Queued,
                    },
                    crate::api::Waiter {
                        node: NodeId(x),
                        on: crate::api::WaitOn::Node(NodeId(self.me), None),
                    },
                ],
                report: Box::default(),
            });
            return Err(self.stop(Wait::Node(x, None)));
        }
        match got {
            Ok((outs, end, fields)) => {
                let vals = outs.iter().map(|o| o.1).collect();
                self.reads.push(Read::Stream { outs, end, fields });
                Ok(vals)
            }
            Err(x) => Err(self.stop(Wait::Node(x, None))),
        }
    }

    /// The nearest previous sibling whose text passes `pred` (never waits).
    #[must_use]
    pub fn prev_sibling_where(&mut self, pred: &dyn Fn(OpId, &[ValueId]) -> bool) -> Option<NodeId> {
        #[cfg(feature = "observe")]
        if self.calls.borrow().is_some() {
            let asked = std::cell::RefCell::new(Vec::new());
            let r = self.prev_sibling_where_inner(&|op, args| {
                let b = pred(op, args);
                asked.borrow_mut().push((op, args.to_vec(), b));
                b
            });
            crate::record::call(&self.calls, "prev_sibling_where", crate::record::j(&asked.into_inner()), crate::record::j(&r));
            return r;
        }
        self.prev_sibling_where_inner(pred)
    }

    #[inline]
    fn prev_sibling_where_inner(&mut self, pred: &dyn Fn(OpId, &[ValueId]) -> bool) -> Option<NodeId> {
        let w = self.w;
        let mut walk = Vec::new();
        let mut ans = None;
        for (x, inert) in w.backward(self.me) {
            let Some(n) = w.node(x) else { break };
            walk.push((x, super::world::text_key(&n.text), inert));
            if !inert && pred(n.text.op, &n.text.args) {
                ans = Some(x);
                break;
            }
        }
        self.reads.push(Read::Prev { walk, ans });
        ans.map(NodeId)
    }

    /// Pushes `x` on sequence `f` in this node's scope.
    pub fn push_contribution(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &x, )) } else { serde_json::Value::Null };
        let r = self.push_contribution_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "push_contribution", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn push_contribution_inner(&mut self, f: Field, x: ValueId) {
        self.define_with_inner(f, x, Locality::Local);
    }

    fn seq(&mut self, f: Field, prev: bool) -> Result<Option<ValueId>, Stop> {
        if super::core::debug() {
            let ops = self.w.seq_ops(self.core, self.me, f);
            eprintln!("seq {} prev {prev}: {:?}", self.me, ops.map(|v| v.iter().map(|o| format!("{o:?}")).collect::<Vec<_>>()));
        }
        match self.w.seq_read(self.core, self.me, f, prev) {
            Ok(val) => {
                self.reads.push(Read::Seq { f, prev, val });
                Ok(val)
            }
            Err(x) => Err(self.stop(Wait::Node(x, Some(f)))),
        }
    }

    /// The last element of sequence `f` in this node's scope.
    ///
    /// # Errors
    ///
    /// `Stop` while an earlier entry of the scope is to be made.
    pub fn prev_contribution(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, )) } else { serde_json::Value::Null };
        let r = self.prev_contribution_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "prev_contribution", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn prev_contribution_inner(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        self.seq(f, true)
    }

    /// Sequence `f` in this node's scope, before it (a sequence value).
    ///
    /// # Errors
    ///
    /// `Stop` while an earlier entry of the scope is to be made.
    pub fn contributions_before_me(&mut self, f: Field) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, )) } else { serde_json::Value::Null };
        let r = self.contributions_before_me_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "contributions_before_me", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn contributions_before_me_inner(&mut self, f: Field) -> Result<ValueId, Stop> {
        Ok(self.seq(f, false)?.unwrap_or(ValueId::NIL))
    }

    /// Takes the last element of sequence `f` (replacing it by `r`): later
    /// readers see it gone; this one gets it.
    ///
    /// # Errors
    ///
    /// `Stop` while an earlier entry of the scope is to be made.
    pub fn take_prev_contribution(&mut self, f: Field, r: Option<ValueId>) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &r, )) } else { serde_json::Value::Null };
        let r = self.take_prev_contribution_inner(f, r);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "take_prev_contribution", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn take_prev_contribution_inner(&mut self, f: Field, r: Option<ValueId>) -> Result<Option<ValueId>, Stop> {
        let got = self.seq(f, true)?;
        let v = &self.core.values;
        let x = match r {
            Some(r) => super::seq::repl(v, r),
            None => super::seq::take(v),
        };
        self.define_with_inner(f, x, Locality::Local);
        Ok(got)
    }

    /// Sets an environment field of the unit this application owns (an
    /// application's run only).
    pub fn set_env(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &x, )) } else { serde_json::Value::Null };
        let r = self.set_env_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "set_env", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn set_env_inner(&mut self, f: Field, x: ValueId) {
        if let Some(&i) = self.ix.get(&(3, f, 0)) {
            self.env[i].1 = x;
        } else {
            self.ix.insert((3, f, 0), self.env.len());
            self.env.push((f, x));
        }
    }

    /// Keeps `token` if this run stops: the next run gets it from
    /// `parked` when its reads so far still hold.
    pub fn park(&mut self, token: Box<dyn Any + Send>) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.park_inner(token);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "park", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn park_inner(&mut self, token: Box<dyn Any + Send>) {
        self.kept = Some(token);
    }

    /// The token a stopped run of this node kept.
    pub fn parked(&mut self) -> Option<Box<dyn Any + Send>> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.parked_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "parked", a, crate::record::j(&r.is_some()));
        r
    }

    #[inline]
    fn parked_inner(&mut self) -> Option<Box<dyn Any + Send>> {
        self.kept.take()
    }

    /// Drops the kept state: the run starts over, and what its first pass
    /// read and made is not this run's.
    pub fn restart(&mut self) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.restart_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "restart", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn restart_inner(&mut self) {
        self.kept = None;
        self.restart = true;
        self.reads.clear();
        self.defs.clear();
        self.contribs.clear();
        self.pubs.clear();
        self.kids.clear();
        self.conts.clear();
        self.effect = None;
        self.kinds.clear();
        self.diag.clear();
        self.claim = None;
        self.env.clear();
        self.ix.clear();
        self.nkids = 0;
    }

    pub fn diagnostic(&mut self, msg: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&msg, )) } else { serde_json::Value::Null };
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
        if let Some(&v) = self
            .core
            .text_memo
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
        {
            return v;
        }
        let v = produce(&self.core.values);
        self.core
            .text_memo
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, v);
        v
    }

    /// Whether this run is speculative.
    #[must_use]
    pub fn is_speculative(&self) -> bool {
        false
    }

    /// The values' epoch: every [`ValueId`] this run makes or reads stays
    /// valid while it is unchanged.
    #[must_use]
    pub fn values_epoch(&self) -> u64 {
        0
    }

    /// Whether `f` is a loop-back field.
    pub fn is_loopback(&mut self, f: Field) -> bool {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, )) } else { serde_json::Value::Null };
        let r = self.is_loopback_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "is_loopback", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn is_loopback_inner(&mut self, f: Field) -> bool {
        let val = self.core.is_loop(f);
        self.reads.push(Read::IsLoop { f, val });
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

/// The node of application `op` on `args`.
pub(crate) fn app_id(op: OpId, args: &[ValueId]) -> Id {
    let mut w: Vec<u64> = args.iter().map(|a| a.raw()).collect();
    w.push(u64::from(op.0));
    // (Never a placed id: those have bit 63; never a pushed one: small.)
    crate::values::hash_words(0xa99, &w) & !(1 << 63) | 1 << 62
}
