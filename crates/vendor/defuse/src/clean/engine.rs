//! The public engine: registration, edits, settings and queries over the
//! current world.

use std::sync::Arc;

use super::build::{Builder, new_unit};
use super::core::{Core, lock};
use super::world::{Id, Place, Pos, St, World};
use crate::api::{
    CapPolicy, Effect, Limits, LoopStart, Monoid, NodeId, NodeInfo, NodeState, OpId, Operation, Settled, Stop, UnitId,
};
use crate::values::{Field, FxHashMap, FxHashSet, ValueId, Values};
use crate::Ctx;

struct OpenOp;

impl Operation for OpenOp {
    fn name(&self) -> &'static str {
        "open"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, crate::api::Locality)>> {
        Some(Vec::new())
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Open
    }
    fn run(&self, _: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

struct CloseOp;

impl Operation for CloseOp {
    fn name(&self) -> &'static str {
        "close"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, crate::api::Locality)>> {
        Some(Vec::new())
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Close
    }
    fn run(&self, _: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

/// Where a pushed or inserted top node goes.
#[derive(Copy, Clone)]
enum Anchor {
    End,
    After(Id),
    Before(Id),
}

/// An incremental evaluator over units of text.
pub struct Engine {
    pub(crate) core: std::sync::Arc<Core>,
    /// The current world (the last round's, between evaluations).
    pub(crate) w: World,
    /// The last evaluation's round worlds, edits since applied to each
    /// (`LoopStart::Initial` and `PerRound` with loop-back fields): every
    /// round's but the last when `last_is_round` (then `w` is it).
    pub(crate) rounds: Vec<World>,
    /// The nodes each kept round world's edits made dirty.
    pub(crate) round_dirty: Vec<Vec<Id>>,
    /// Whether `w` is the last round's world (no final phase ran).
    pub(crate) last_is_round: bool,
    /// During an evaluation: the old round worlds still to run, the rounds
    /// done, and the old final world, with the nodes edits made dirty in
    /// each (an edit absorbed meanwhile reaches them all).
    pub(crate) ev_olds: std::collections::VecDeque<(World, Vec<Id>)>,
    pub(crate) ev_done: Vec<(World, Vec<Id>)>,
    pub(crate) ev_fin: Option<(World, Vec<Id>)>,
    /// The world of the round just finished: a node whose result there
    /// still holds here takes it without running.
    pub(crate) ev_prev: Option<World>,
    /// Marked nodes reported settled: at which version, with which output.
    pub(crate) reported: FxHashMap<Id, (u64, ValueId)>,
    pub(crate) next_id: Id,
    pub(crate) units: u32,
    pub(crate) loop_start: LoopStart,
    pub(crate) max_rounds: u64,
    pub(crate) cap_policy: CapPolicy,
    /// The loop-back fields that take their round values early
    /// (`declare_loopback_early`).
    pub(crate) early: FxHashSet<Field>,
    /// This pass: whether early values are taken (not in the final phase),
    /// each early field's value changes, and those capped (kept as they
    /// are).
    pub(crate) early_live: bool,
    pub(crate) early_changes: FxHashMap<Field, u32>,
    pub(crate) early_frozen: FxHashSet<Field>,
    /// Whether evaluations audit their runs (`set_run_audit`), and the
    /// running evaluation's audit.
    pub(crate) audit_on: bool,
    pub(crate) audit: Option<crate::api::RunAudit>,
    pub(crate) limits: Limits,
    pub(crate) roots: Vec<NodeId>,
    pub(crate) version: u64,
    /// Nodes to run (dirty) found by edits since the last evaluation.
    pub(crate) pending: Vec<Id>,
    /// Kept tokens of stopped runs (`Ctx::park`).
    /// with the reads the stopped run made (it resumes while they hold).
    pub(crate) kept: FxHashMap<Id, super::ctx::KeptState>,
    /// The kept states' reads indexed by what may break them.
    pub(crate) kept_ix: FxHashMap<Id, super::eval::KeptIx>,
    /// Continuing placers' placements as their last stop left them.
    pub(crate) cont_memo: FxHashMap<Id, super::build::ContMemo>,
    /// The text each kept state's token was parked under.
    pub(crate) kept_text: FxHashMap<Id, std::sync::Arc<super::world::Text>>,
    pub(crate) marked: FxHashSet<Id>,
    pub(crate) settled_out: Vec<Settled>,
    pub(crate) edits: u64,
    pub(crate) created: u64,
    pub(crate) freed: u64,
    pub(crate) rebinds: u64,
    /// (`Report::kept_asked`, `Report::kept_held`.)
    pub(crate) kept_asked: u64,
    pub(crate) kept_held: u64,
    pub(crate) rerun_text: Vec<Id>,
    /// Whether evaluations compute heights and the critical chain.
    pub(crate) heights: bool,
    /// While a step merges: the field writes recorded ([`super::batch`]).
    pub(crate) defer: Option<super::batch::Pending>,
    /// Time in runs, in commits and workers idle this evaluation (ns), and
    /// how much of it the rounds before took.
    pub(crate) t_op: u64,
    pub(crate) t_commit: u64,
    pub(crate) t_idle: u64,
    pub(crate) t_seen: (u64, u64, u64),
    /// Runs this evaluation that stopped (attempts, not runs, in the
    /// report).
    pub(crate) stopped: u64,
    /// The last evaluation's critical chain, highest first.
    pub(crate) chain: Vec<(NodeId, u32, crate::api::Cause)>,
    /// Its runs' queueing, start, end and commit (ns since the evaluation
    /// started).
    pub(crate) chain_at: Vec<(NodeId, [u64; 4])>,
    /// The last evaluation's chain by time (`Engine::critical_chain_by_time`).
    pub(crate) chain_time: Vec<(NodeId, crate::api::Cause, [u64; 4])>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    #[must_use]
    pub fn new() -> Engine {
        let mut eng = Engine {
            core: std::sync::Arc::new(Core::new()),
            w: World::default(),
            rounds: Vec::new(),
            round_dirty: Vec::new(),
            last_is_round: false,
            ev_olds: std::collections::VecDeque::new(),
            ev_done: Vec::new(),
            ev_fin: None,
            ev_prev: None,
            reported: FxHashMap::default(),
            next_id: 1,
            units: 0,
            loop_start: LoopStart::Initial,
            max_rounds: 64,
            cap_policy: CapPolicy::Error,
            early: FxHashSet::default(),
            early_live: false,
            early_changes: FxHashMap::default(),
            early_frozen: FxHashSet::default(),
            audit_on: false,
            audit: None,
            limits: Limits::default(),
            roots: Vec::new(),
            version: 0,
            pending: Vec::new(),
            kept: FxHashMap::default(),
            kept_ix: FxHashMap::default(),
            cont_memo: FxHashMap::default(),
            kept_text: FxHashMap::default(),
            marked: FxHashSet::default(),
            settled_out: Vec::new(),
            edits: 0,
            created: 0,
            freed: 0,
            rebinds: 0,
            kept_asked: 0,
            kept_held: 0,
            rerun_text: Vec::new(),
            heights: true,
            defer: None,
            t_op: 0,
            t_commit: 0,
            t_idle: 0,
            t_seen: (0, 0, 0),
            stopped: 0,
            chain: Vec::new(),
            chain_at: Vec::new(),
            chain_time: Vec::new(),
        };
        eng.register(Arc::new(OpenOp));
        eng.register(Arc::new(CloseOp));
        eng
    }

    #[must_use]
    pub fn values(&self) -> &Values {
        &self.core.values
    }

    pub fn register(&mut self, op: Arc<dyn Operation>) -> OpId {
        #[cfg(feature = "observe")]
        #[allow(clippy::cast_possible_truncation)]
        // The two built-in operations are the engine's own: not recorded.
        let op: Arc<dyn Operation> = if crate::record::rec().is_some() && self.core.ops.len() >= 2 {
            let id = OpId(self.core.ops.len() as u32);
            crate::record::api(&self.core.values, "register", serde_json::json!([op.name()]), crate::record::j(&id));
            Arc::new(crate::record::RecOp { inner: op, id })
        } else {
            op
        };
        let name = op.name();
        self.core_mut().ops.push(op);
        self.core_mut().op_names.push(name);
        self.core_mut().op_reads.push(std::sync::atomic::AtomicU32::new(0));
        #[allow(clippy::cast_possible_truncation)]
        OpId((self.core.ops.len() - 1) as u32)
    }

    /// The read path of `steps` ([`crate::PathId`]; for
    /// [`Ctx::use_field_path`], [`Ctx::acc_fold_path`]). The same steps give
    /// the same path.
    pub fn path(&self, steps: &[crate::api::Step]) -> crate::api::PathId {
        crate::api::PathId(self.core.intern_path(steps))
    }

    /// Registers a projection for [`Ctx::use_field_proj`].
    ///
    /// # Panics
    ///
    /// If more than 65,533 projections are registered.
    pub fn register_projection(&mut self, p: crate::api::Projection) -> crate::api::ProjId {
        #[cfg(feature = "observe")]
        #[allow(clippy::cast_possible_truncation)]
        let p = {
            let id = (self.core.projs.len() + 2) as u16;
            crate::record::api(&self.core.values, "register_projection", serde_json::Value::Null, serde_json::json!(id));
            crate::record::rec_projection(id, p)
        };
        self.core_mut().projs.push(p);
        crate::api::ProjId(u16::try_from(self.core.projs.len() + 1).expect("too many projections"))
    }

    /// The built-in operation that opens a scope after its node.
    #[must_use]
    pub fn open_op(&self) -> OpId {
        OpId(0)
    }

    /// The built-in operation that closes the scope it is in.
    #[must_use]
    pub fn close_op(&self) -> OpId {
        OpId(1)
    }

    pub fn new_unit(&mut self) -> UnitId {
        let u = self.core.alloc_unit();
        self.units += 1;
        self.each_world(|w| new_unit(w, u, super::world::NONE));
        #[cfg(feature = "observe")]
        self.rec_call("new_unit", crate::record::rec().map(|_| crate::record::j(&())), &UnitId(u));
        UnitId(u)
    }

    /// Writes an engine call when recording (`a`: its arguments, taken
    /// before the call moved them) with its result, as `play` reads it.
    #[cfg(feature = "observe")]
    fn rec_call<R: serde::Serialize>(&self, f: &str, a: Option<serde_json::Value>, r: &R) {
        if let Some(a) = a {
            crate::record::api(&self.core.values, f, a, crate::record::j(r));
        }
    }

    fn fresh(&mut self) -> Id {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Applies `f` to the current world and every kept round world.
    fn each_world(&mut self, f: impl Fn(&mut World)) {
        for w in &mut self.rounds {
            f(w);
        }
        f(&mut self.w);
    }

    /// Applies `f` to a builder on every kept round world and on the
    /// current world, and keeps what it made.
    fn edit<T>(&mut self, f: impl Fn(&mut Builder<'_>) -> T) -> T {
        self.edits += 1;
        for (i, w) in self.rounds.iter_mut().enumerate() {
            let mut b = Builder::new(&self.core, std::mem::take(w));
            b.defer_claims = true;
            f(&mut b);
            let m = b.finish();
            *w = m.w;
            self.round_dirty[i].extend(m.dirty);
        }
        for (w, d) in self.ev_olds.iter_mut().chain(self.ev_done.iter_mut()).chain(self.ev_fin.iter_mut()) {
            let mut b = Builder::new(&self.core, std::mem::take(w));
            b.defer_claims = true;
            f(&mut b);
            let m = b.finish();
            *w = m.w;
            d.extend(m.dirty);
        }
        let w = std::mem::take(&mut self.w);
        let mut b = Builder::new(&self.core, w);
        b.defer_claims = true;
        let r = f(&mut b);
        let m = b.finish();
        self.w = m.w;
        self.pending.extend(m.dirty);
        self.created += m.created;
        self.rebinds += m.rebinds;
        self.freed += m.freed.len() as u64;
        for x in m.freed {
            self.kept.remove(&x);
        }
        r
    }

    /// A top node with text `(op, args)` placed at `at` in unit `u`.
    fn make_top(&mut self, u: u32, at: Anchor, op: OpId, args: Vec<ValueId>) -> NodeId {
        let id = self.fresh();
        let text = self.core.text(Some(id), op, Arc::from(args));
        self.edit(|b| {
            let after = match at {
                Anchor::End => b.w.unit(u).order.last().map_or(0, |(&p, _)| p),
                // (After its extent: its subtree and the continuations it
                // placed, where a cold build puts the next node.)
                Anchor::After(x) => {
                    let end = b.extent_end(x);
                    b.w.node(end).or_else(|| b.w.node(x)).map_or(0, |e| e.pos)
                }
                Anchor::Before(x) => {
                    let p = b.w.node(x).map_or(0, |n| n.pos);
                    b.w.unit(u).order.below(&p).map_or(0, |(&p, _)| p)
                }
            };
            let pos = b.top_position(u, after);
            b.create(id, text.clone(), u, super::world::NONE, Place::Top, 0, pos);
            b.frames_from(u, pos);
            b.file_declared_pub(id);
        });
        self.rerun_text.push(id);
        NodeId(id)
    }

    /// Appends a node at the end of `unit`.
    pub fn push(&mut self, unit: UnitId, op: OpId, args: Vec<ValueId>) -> NodeId {
        #[cfg(feature = "observe")]
        let a = crate::record::rec().map(|_| crate::record::j(&(&unit, &op, &args)));
        let r = self.make_top(unit.0, Anchor::End, op, args);
        #[cfg(feature = "observe")]
        self.rec_call("push", a, &r);
        r
    }

    /// Appends `n` nodes to the end of `unit`, in order (recorded as the
    /// pushes it is).
    pub fn push_many(&mut self, unit: UnitId, op: OpId, args: Vec<Vec<ValueId>>) -> Vec<NodeId> {
        args.into_iter().map(|a| self.push(unit, op, a)).collect()
    }

    /// Keeps the node's applications alive across collections.
    pub fn add_root(&mut self, n: NodeId) {
        self.roots.push(n);
        #[cfg(feature = "observe")]
        self.rec_call("add_root", crate::record::rec().map(|_| crate::record::j(&(&n,))), &());
    }

    pub fn remove_root(&mut self, node: NodeId) {
        self.roots.retain(|&r| r != node);
        #[cfg(feature = "observe")]
        self.rec_call("remove_root", crate::record::rec().map(|_| crate::record::j(&(&node,))), &());
    }

    /// # Panics
    ///
    /// If `node` is not a live node.
    pub fn insert_after(&mut self, node: NodeId, op: OpId, args: Vec<ValueId>) -> NodeId {
        let u = self.w.node(node.0).expect("insert_after: a live node").unit;
        #[cfg(feature = "observe")]
        let a = crate::record::rec().map(|_| crate::record::j(&(&node, &op, &args)));
        let r = self.make_top(u, Anchor::After(node.0), op, args);
        #[cfg(feature = "observe")]
        self.rec_call("insert_after", a, &r);
        r
    }

    /// # Panics
    ///
    /// If `node` is not a live node.
    pub fn insert_before(&mut self, node: NodeId, op: OpId, args: Vec<ValueId>) -> NodeId {
        let u = self.w.node(node.0).expect("insert_before: a live node").unit;
        #[cfg(feature = "observe")]
        let a = crate::record::rec().map(|_| crate::record::j(&(&node, &op, &args)));
        let r = self.make_top(u, Anchor::Before(node.0), op, args);
        #[cfg(feature = "observe")]
        self.rec_call("insert_before", a, &r);
        r
    }

    /// Replaces a node's text; the node keeps its id.
    pub fn replace(&mut self, node: NodeId, op: OpId, args: Vec<ValueId>) {
        #[cfg(feature = "observe")]
        let a = crate::record::rec().map(|_| crate::record::j(&(&node, &op, &args)));
        self.replace_inner(node, op, args);
        #[cfg(feature = "observe")]
        self.rec_call("replace", a, &());
    }

    fn replace_inner(&mut self, node: NodeId, op: OpId, args: Vec<ValueId>) {
        if self.w.node(node.0).is_none() {
            return;
        }
        let text = self.core.text(Some(node.0), op, Arc::from(args));
        self.edit(|b| b.retext(node.0, text.clone()));
        self.rerun_text.push(node.0);
    }

    /// Removes a node and its subtree.
    pub fn remove(&mut self, node: NodeId) {
        if self.w.node(node.0).is_some() {
            self.edit(|b| b.free(node.0));
            self.roots.retain(|&r| r != node);
        }
        #[cfg(feature = "observe")]
        self.rec_call("remove", crate::record::rec().map(|_| crate::record::j(&(&node,))), &());
    }

    /// The committed output of a node (`None`: not run, claimed or gone).
    #[must_use]
    pub fn output(&self, n: NodeId) -> Option<ValueId> {
        self.w.node(n.0).and_then(super::world::Node::out)
    }

    /// A node's children (scope markers left out).
    #[must_use]
    pub fn children(&self, n: NodeId) -> Vec<NodeId> {
        let Some(x) = self.w.node(n.0) else { return Vec::new() };
        let Some(r) = &x.res else { return Vec::new() };
        r.kids
            .iter()
            .chain(&r.conts)
            .filter(|&&k| self.w.node(k).is_some_and(|c| !matches!(c.place, Place::Marker(_))))
            .map(|&k| NodeId(k))
            .collect()
    }

    /// The siblings a node claims.
    #[must_use]
    pub fn claimed(&self, n: NodeId) -> Vec<NodeId> {
        let mut v: Vec<(Pos, Id)> = self
            .w
            .nodes
            .iter()
            .filter(|(_, x)| x.st == St::Inert(n.0))
            .map(|(&id, x)| (x.pos, id))
            .collect();
        v.sort_unstable();
        v.into_iter().map(|x| NodeId(x.1)).collect()
    }

    /// The node whose definition of `f` reaches `n`.
    #[must_use]
    pub fn reaching_def(&self, n: NodeId, f: Field) -> Option<NodeId> {
        let (u, pos, fr) = self.w.frame_at(n.0, false)?;
        self.w.reaching(u, f, pos, fr, n.0).map(|(_, e)| NodeId(e.node))
    }

    /// The value `n` defines for `f` (constant time after the first ask of
    /// a node with many definitions).
    #[must_use]
    pub fn def_value(&self, n: NodeId, f: Field) -> Option<ValueId> {
        self.w.node(n.0)?.res.as_ref()?.def_value(f)
    }

    /// Every field `n` defines with its value (as [`Self::def_value`] gives
    /// each: a field defined at more than one locality, the first made),
    /// in the order it made them: a node's final values in one pass.
    #[must_use]
    pub fn defs_of(&self, n: NodeId) -> Vec<(Field, ValueId)> {
        let Some(res) = self.w.node(n.0).and_then(|x| x.res.clone()) else { return Vec::new() };
        let mut seen = FxHashSet::default();
        res.defs.iter().filter(|d| seen.insert(d.f)).map(|d| (d.f, d.val)).collect()
    }

    /// The fold of accumulator `f` over `unit`.
    #[must_use]
    pub fn acc_value(&self, unit: UnitId, f: Field) -> Option<ValueId> {
        let mo = self.core.acc(f)?;
        let m = super::world::AccMeasure {
            m: &**mo,
            v: &self.core.values,
        };
        Some(self.w.fold(&m, unit.0, f, 0, Pos::MAX))
    }

    pub fn declare_accumulator(&mut self, f: Field, m: Arc<dyn Monoid>) {
        #[cfg(feature = "observe")]
        let m: Arc<dyn Monoid> = {
            crate::record::api(&self.core.values, "declare_accumulator", crate::record::j(&f), crate::record::monoid_mode(&*m));
            Arc::new(crate::record::RecMonoid { inner: m, field: f })
        };
        self.core_mut().accs.insert(f, m);
        // (What nodes made contribute to it before goes in now.)
        if !self.w.nodes.is_empty() {
            self.edit(|b| b.refile_contribs());
        }
    }

    /// Declares every field whose name starts with `prefix` (and has no
    /// accumulator of its own) an accumulator with monoid `m`, as
    /// [`Engine::declare_accumulator`] each would; the first declared
    /// prefix that matches wins.
    pub fn declare_accumulator_prefix(&mut self, prefix: &str, m: Arc<dyn Monoid>) {
        #[cfg(feature = "observe")]
        crate::record::api(&self.core.values, "declare_accumulator_prefix", crate::record::j(&(prefix,)), crate::record::monoid_mode(&*m));
        let core = self.core_mut();
        core.acc_prefixes.push((prefix.to_string(), m));
        // (Fields resolved before see the new prefix.)
        core.acc_memo = crate::sync::Table::new();
        if !self.w.nodes.is_empty() {
            self.edit(|b| b.refile_contribs());
        }
    }

    /// Marks `f` stable (a hint; the clean core needs none).
    pub fn set_stable(&mut self, f: Field) {
        self.core_mut().stable.insert(f);
        #[cfg(feature = "observe")]
        self.rec_call("set_stable", crate::record::rec().map(|_| crate::record::j(&(&f,))), &());
    }

    /// Whether evaluations compute heights and the critical chain.
    pub fn set_heights(&mut self, on: bool) {
        self.heights = on;
        #[cfg(feature = "observe")]
        self.rec_call("set_heights", crate::record::rec().map(|_| crate::record::j(&(&on,))), &());
    }

    /// Readers of field `f` wait on a node still in work whose pending
    /// definition set (`Operation::defs` `None`) may hold `f`, before them,
    /// even when the scheduler predicts (which otherwise reads past it, a
    /// guess its definition re-asks): for a field such a node usually does
    /// change, read early by many (a preamble's category codes, say). Their
    /// read hints (`Operation::reads`) hold them back likewise.
    pub fn wait_on_pending(&mut self, f: Field) {
        self.core_mut().wait_pending.insert(f);
        #[cfg(feature = "observe")]
        self.rec_call("wait_on_pending", crate::record::rec().map(|_| crate::record::j(&(&f,))), &());
    }

    /// Bounds what a pending claimer (`Claim::Pending`) without a reach of
    /// its own (`Operation::claim_limit`, or a smaller one) holds back to
    /// the `w` siblings after it: those past them run before its claim is
    /// known, as a guess that it claims no more, taken back when it does
    /// (`Report::claims_past_reach`). `None` (the default): it holds back
    /// all after it.
    pub fn set_claim_window(&mut self, w: Option<usize>) {
        self.core_mut().claim_window = w;
        #[cfg(feature = "observe")]
        self.rec_call("set_claim_window", crate::record::rec().map(|_| crate::record::j(&(&w,))), &());
    }

    /// With a claim window (`set_claim_window`), a sibling that is itself a
    /// pending claimer is not held by the pending claimer before it at
    /// all: guessed not claimed, taken back when it is
    /// (`Report::claimed_after_run`), so a run of pending claimers goes on
    /// together. Off by default.
    pub fn set_claim_chain(&mut self, on: bool) {
        self.core_mut().claim_chain = on;
        #[cfg(feature = "observe")]
        self.rec_call("set_claim_chain", crate::record::rec().map(|_| crate::record::j(&(&on,))), &());
    }

    /// Opts field `f` in to guessing from the last evaluation: a reader
    /// that would wait on a definer declared, not yet made, of `f` takes
    /// the value its result from a previous evaluation read at the same
    /// point, as a guess the merge validates (the definer's commit re-asks
    /// it). When predicting only; a result of the evaluation running is
    /// never guessed from, so a cold build is as without it.
    pub fn guess_from_last(&mut self, f: Field) {
        self.core_mut().guess_last.insert(f);
        #[cfg(feature = "observe")]
        self.rec_call("guess_from_last", crate::record::rec().map(|_| crate::record::j(&(&f,))), &());
    }

    pub fn declare_sequence(&mut self, f: Field) {
        self.core_mut().seqs.insert(f);
        #[cfg(feature = "observe")]
        self.rec_call("declare_sequence", crate::record::rec().map(|_| crate::record::j(&(&f,))), &());
    }

    pub fn declare_loopback(&mut self, f: Field, init: ValueId) {
        self.declare_loopbacks_inner(&[(f, init)]);
        #[cfg(feature = "observe")]
        self.rec_call("declare_loopback", crate::record::rec().map(|_| crate::record::j(&(&f, &init))), &());
    }

    /// Audits every evaluation's runs ([`crate::Report::audit`]): each run
    /// of a node that had a result is compared with that result, and one
    /// whose effects outside the run (output, definitions, contributions,
    /// publications, scope effect and kinds, placements, diagnostics,
    /// claim) all came out the same is reported wasted, with what made it
    /// run again and its time. Off by default (the comparison costs a
    /// little per commit).
    pub fn set_run_audit(&mut self, on: bool) {
        self.audit_on = on;
        #[cfg(feature = "observe")]
        self.rec_call("set_run_audit", crate::record::rec().map(|_| crate::record::j(&(&on,))), &());
    }

    /// Declares loop-back field `f` (as [`Self::declare_loopback`]; a
    /// field declared already keeps its initial value) and opts it in to
    /// early round values: when a commit changes the value published for
    /// `f`, its readers (and the definitions forwarding it,
    /// `Ctx::define_forward`) take that value at once, not when the round
    /// ends. The round ends as before, when nothing moves; a field moved
    /// early does not move at its end.
    ///
    /// # Determinism
    ///
    /// This is chaotic (asynchronous) iteration, not rounds: readers of
    /// the next value run while the round still runs, and what they give
    /// feeds it. It is **not deterministic**: when the fields' loop has
    /// one fixed point and the rounds reach it, the result is the rounds'
    /// result; otherwise it can depend on the schedule (the thread count
    /// among it) and differ from the rounds'. A loop that oscillates, or
    /// has more than one fixed point, can settle on another fixed point, or
    /// on one where the rounds would not settle at all. With no field
    /// declared early, evaluations are as without this (deterministic).
    /// With one, every evaluation starts the loop-back fields from their
    /// last values ([`crate::LoopStart::Previous`], whatever
    /// `set_loop_start` says): rounds are not kept to start again from.
    /// [`Report::early_loops`] says, per field, how often it changed and
    /// whether its changes reached the round cap
    /// ([`EarlyLoop::capped`], [`EarlyLoop::oscillated`]); the cap counts
    /// each field's changes, as rounds do.
    ///
    /// [`Report::early_loops`]: crate::Report::early_loops
    /// [`EarlyLoop::capped`]: crate::EarlyLoop::capped
    /// [`EarlyLoop::oscillated`]: crate::EarlyLoop::oscillated
    pub fn declare_loopback_early(&mut self, f: Field, init: ValueId) {
        self.declare_loopbacks_inner(&[(f, init)]);
        self.early.insert(f);
        #[cfg(feature = "observe")]
        self.rec_call("declare_loopback_early", crate::record::rec().map(|_| crate::record::j(&(&f, &init))), &());
    }

    /// Declares several loop-back fields at once: one pass over the runs
    /// (a field declared already is left as it is).
    pub fn declare_loopbacks(&mut self, fields: &[(Field, ValueId)]) {
        self.declare_loopbacks_inner(fields);
        #[cfg(feature = "observe")]
        self.rec_call("declare_loopbacks", crate::record::rec().map(|_| crate::record::j(&(fields,))), &());
    }

    fn declare_loopbacks_inner(&mut self, fields: &[(Field, ValueId)]) {
        let mut new: FxHashMap<Field, ValueId> = FxHashMap::default();
        for &(f, init) in fields {
            if !self.core.loops.contains_key(&f) {
                new.entry(f).or_insert(init);
            }
        }
        if new.is_empty() {
            return;
        }
        self.core_mut().loops.extend(new.iter().map(|(&f, &x)| (f, x)));
        self.each_world(|w| {
            for (&f, &init) in &new {
                if w.loops.get(&f).is_none() {
                    w.note(super::world::Change::Loop(f));
                    w.loops.insert(f, init, &super::ptree::NoMeasure);
                }
            }
        });
        // Runs that read one, or asked whether it is a loop-back, run again.
        self.edit(|b| {
            let hit: Vec<(Id, Field)> = b
                .w
                .nodes
                .iter()
                .filter_map(|(&id, n)| {
                    n.res.as_ref()?.reads.iter().find_map(|rd| match rd {
                        super::world::Read::Use { f, .. } | super::world::Read::IsLoop { f, .. } if new.contains_key(f) => Some((id, *f)),
                        _ => None,
                    })
                })
                .collect();
            for (id, f) in hit {
                b.make_dirty_for(id, crate::api::Rerun::Loop { field: f });
            }
        });
    }

    pub fn set_loop_start(&mut self, s: LoopStart) {
        self.loop_start = s;
        self.core.hold_fin.store(s != LoopStart::Previous, std::sync::atomic::Ordering::Relaxed);
        #[cfg(feature = "observe")]
        self.rec_call("set_loop_start", crate::record::rec().map(|_| crate::record::j(&(&s,))), &());
    }

    pub fn set_max_rounds(&mut self, n: u64) {
        self.max_rounds = n;
        #[cfg(feature = "observe")]
        self.rec_call("set_max_rounds", crate::record::rec().map(|_| crate::record::j(&(&n,))), &());
    }

    /// What reaching the round cap does ([`crate::CapPolicy`]; default
    /// `Error`).
    pub fn set_round_cap_policy(&mut self, p: CapPolicy) {
        self.cap_policy = p;
        #[cfg(feature = "observe")]
        self.rec_call("set_round_cap_policy", crate::record::rec().map(|_| crate::record::j(&(&p,))), &());
    }

    /// Bounds an evaluation (see [`Limits`]); exceeding one is an error.
    pub fn set_limits(&mut self, l: Limits) {
        #[cfg(feature = "observe")]
        let a = crate::record::rec().map(|_| crate::record::j(&(&l,)));
        self.limits = l;
        #[cfg(feature = "observe")]
        self.rec_call("set_limits", a, &());
    }

    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Supplies environment fields (replacing the previous ones).
    pub fn set_environment(&mut self, env: impl IntoIterator<Item = (Field, ValueId)>) {
        // (A read no definition reaches reads the environment: kept
        // states ask all their reads again.)
        self.w.note(super::world::Change::All);
        let new: FxHashMap<Field, ValueId> = env.into_iter().collect();
        #[cfg(feature = "observe")]
        {
            let mut v: Vec<(Field, ValueId)> = new.iter().map(|(&f, &x)| (f, x)).collect();
            v.sort_unstable();
            crate::record::api(&self.core.values, "set_environment", crate::record::j(&v), serde_json::Value::Null);
        }
        // By answer: a field whose answer stays (the provider's underneath
        // one no longer supplied, say) wakes no reader.
        let changed = {
            let mut e = self.core.env.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            let fs: FxHashSet<Field> = e.known.keys().chain(new.keys()).copied().collect();
            let before: Vec<(Field, Option<Option<ValueId>>)> = fs.into_iter().map(|f| (f, self.core.env_answer(&e, f))).collect();
            e.known = new;
            before.into_iter().filter(|(f, b)| self.core.env_answer(&e, *f) != *b).map(|(f, _)| f).collect::<Vec<_>>()
        };
        self.env_changed(&changed);
    }

    /// Declares environment fields absent (a read gets `None` instead of
    /// waiting).
    pub fn set_environment_absent(&mut self, absent: impl IntoIterator<Item = Field>) {
        self.w.note(super::world::Change::All);
        let new: FxHashSet<Field> = absent.into_iter().collect();
        #[cfg(feature = "observe")]
        {
            let mut v: Vec<Field> = new.iter().copied().collect();
            v.sort_unstable();
            crate::record::api(&self.core.values, "set_environment_absent", crate::record::j(&v), serde_json::Value::Null);
        }
        let changed = {
            let mut e = self.core.env.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            let fs: Vec<Field> = new.symmetric_difference(&e.absent).copied().collect();
            let before: Vec<(Field, Option<Option<ValueId>>)> = fs.into_iter().map(|f| (f, self.core.env_answer(&e, f))).collect();
            e.absent = new;
            before.into_iter().filter(|(f, b)| self.core.env_answer(&e, *f) != *b).map(|(f, _)| f).collect::<Vec<_>>()
        };
        self.env_changed(&changed);
    }

    /// The readers of changed environment fields run again.
    fn env_changed(&mut self, fields: &[Field]) {
        if fields.is_empty() {
            return;
        }
        let fs: FxHashSet<Field> = fields.iter().copied().collect();
        self.edit(|b| {
            // (Each with the changed field it read, the first: its label.)
            let mut hit = Vec::new();
            for (&id, n) in b.w.nodes.iter() {
                let Some(r) = &n.res else { continue };
                if let Some(f) = r.reads.iter().find_map(|rd| match rd {
                    super::world::Read::Env { f, .. } => fs.contains(f).then_some(*f),
                    super::world::Read::Use { f, site, .. } => (*site == super::world::NONE && fs.contains(f)).then_some(*f),
                    super::world::Read::Stream { fields, .. } => fields.iter().find(|(f, _)| fs.contains(f)).map(|&(f, _)| f),
                    _ => None,
                }) {
                    hit.push((id, f));
                }
            }
            for (id, f) in hit {
                b.make_dirty_for(id, crate::api::Rerun::Environment { field: f });
            }
        });
    }

    /// Sets an environment field of an application's unit.
    pub fn set_env(&mut self, unit: UnitId, field: Field, value: ValueId) {
        self.w.note(super::world::Change::All);
        self.edit(|b| b.set_unit_env(unit.0, field, value));
        #[cfg(feature = "observe")]
        self.rec_call("set_env", crate::record::rec().map(|_| crate::record::j(&(&unit, &field, &value))), &());
    }

    /// The environment's supplied fields.
    pub fn environment(&self) -> impl Iterator<Item = (Field, ValueId)> + '_ {
        let e = self.core.env.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut v: Vec<(Field, ValueId)> = e.known.iter().map(|(f, v)| (*f, *v)).collect();
        // The provider's answers stay with it.
        v.extend(e.provided.iter().filter(|(f, _)| !e.known.contains_key(f)).filter_map(|(f, x)| Some((*f, (*x)?))));
        v.into_iter()
    }

    /// Lays `base` under the supplied environment: a field neither
    /// supplied nor declared absent reads its value there (asked at most
    /// once per field for the engine's life, its answer kept through every
    /// evaluation and `set_environment`), else (by `Ctx::use_env`) the
    /// provider's. As if
    /// `base`'s whole domain were supplied, the supplied entries over it,
    /// made as read. Set it before the first evaluation; `environment`
    /// leaves it out.
    pub fn set_environment_base(&mut self, base: Arc<crate::api::EnvBase>) {
        self.w.note(super::world::Change::All);
        #[cfg(feature = "observe")]
        let base = {
            crate::record::api(&self.core.values, "set_environment_base", serde_json::Value::Null, serde_json::Value::Null);
            crate::record::rec_base(base)
        };
        let core = self.core_mut();
        core.base = Some(base);
        core.base_memo = crate::sync::Table::new();
    }

    pub fn set_env_provider(&mut self, provider: Box<crate::api::EnvProvider>) {
        self.w.note(super::world::Change::All);
        #[cfg(feature = "observe")]
        let provider = {
            crate::record::api(&self.core.values, "set_env_provider", serde_json::Value::Null, serde_json::Value::Null);
            crate::record::rec_provider(provider)
        };
        self.core_mut().provider = Some(provider);
    }

    pub fn clear_env_provider(&mut self) {
        self.w.note(super::world::Change::All);
        self.core_mut().provider = None;
        #[cfg(feature = "observe")]
        self.rec_call("clear_env_provider", crate::record::rec().map(|_| crate::record::j(&())), &());
    }

    /// (Values are never swept here: every id stays valid.)
    #[cfg_attr(not(feature = "observe"), allow(unused_variables))]
    pub fn pin(&mut self, v: ValueId) {
        #[cfg(feature = "observe")]
        self.rec_call("pin", crate::record::rec().map(|_| crate::record::j(&(&v,))), &());
    }

    #[cfg_attr(not(feature = "observe"), allow(unused_variables))]
    pub fn unpin(&mut self, v: ValueId) {
        #[cfg(feature = "observe")]
        self.rec_call("unpin", crate::record::rec().map(|_| crate::record::j(&(&v,))), &());
    }

    #[must_use]
    pub fn values_epoch(&self) -> u64 {
        0
    }

    #[must_use]
    pub fn is_live(&self, node: NodeId) -> bool {
        self.w.node(node.0).is_some()
    }

    #[must_use]
    pub fn debug_reads(&self, id: NodeId) -> String {
        self.w
            .node(id.0)
            .and_then(|n| n.res.as_ref())
            .map_or_else(String::new, |r| format!("{:?}", r.reads))
    }

    /// The last evaluation's dependency graph (for scheduling studies):
    /// each committed node with the nodes its result rests on, by what it
    /// read (definers, outputs, anchors, contributors, siblings, the
    /// openers of the frames it read the kind of) and by who placed it.
    #[doc(hidden)]
    #[must_use]
    pub fn dependency_graph(&self) -> Vec<(NodeId, Vec<NodeId>)> {
        use super::world::{FrameId, NONE, Read};
        let w = &self.w;
        let opener = |fr: FrameId| (!fr.is_root()).then_some(fr.0 >> 8);
        let mut out = Vec::new();
        for (&id, n) in w.nodes.iter() {
            let Some(r) = n.res.as_ref() else { continue };
            if matches!(n.place, Place::Marker(_)) {
                continue;
            }
            let mut d: Vec<Id> = Vec::new();
            if n.parent != NONE {
                d.push(n.parent);
            }
            for rd in &r.reads {
                match rd {
                    Read::Use { site, .. } => d.push(*site),
                    Read::Out { node, .. } | Read::Placed { node, .. } | Read::At { node, .. } => d.push(*node),
                    Read::Kind { at, .. } => d.extend(w.node(*at).and_then(|x| opener(x.frame))),
                    Read::Range { f, from, to, past, .. } => {
                        if let Some((ru, lo, hi)) = w.window(id, *from, *to, *past)
                            && let Some(m) = w.field(ru, *f).map(|c| &c.contribs)
                        {
                            let order = &w.unit(ru).order;
                            d.extend(
                                m.range(std::ops::Bound::Excluded(&lo), std::ops::Bound::Excluded(&hi))
                                    .filter_map(|(p, _)| order.get(p).copied()),
                            );
                        }
                    }
                    Read::Seq { f, .. } => {
                        if let Some((_, e)) = w.reaching(n.unit, *f, n.pos, n.frame, id) {
                            d.push(e.node);
                        }
                    }
                    Read::Sib { ans, .. } => d.extend(ans.map(|(x, _)| x)),
                    Read::Claimed { list } => d.extend(list.iter().map(|&(x, _)| x)),
                    Read::Prev { ans, .. } => d.extend(*ans),
                    Read::Stream { outs, fields, .. } => {
                        d.extend(outs.iter().map(|&(x, _)| x));
                        d.extend(fields.iter().map(|&(_, x)| x));
                    }
                    Read::Loop { .. } | Read::Env { .. } | Read::IsLoop { .. } => {}
                }
            }
            d.retain(|&x| x != NONE && x != id && w.node(x).is_some());
            d.sort_unstable();
            d.dedup();
            out.push((NodeId(id), d.into_iter().map(NodeId).collect()));
        }
        out
    }

    #[must_use]
    pub fn debug_node(&self, id: NodeId) -> String {
        self.w.node(id.0).map_or_else(String::new, |n| format!("{n:?}"))
    }

    /// The core, to change (between evaluations: no run holds it then).
    fn core_mut(&mut self) -> &mut Core {
        std::sync::Arc::get_mut(&mut self.core).expect("the core is changed between evaluations")
    }

    /// Marks every node created with `op` from now on (the client's
    /// output nodes, say), at creation.
    pub fn mark_op(&mut self, op: OpId) {
        self.core.marked_ops.write().unwrap_or_else(std::sync::PoisonError::into_inner).insert(op);
        #[cfg(feature = "observe")]
        self.rec_call("mark_op", crate::record::rec().map(|_| crate::record::j(&(&op,))), &());
    }

    #[must_use]
    pub fn live_nodes(&self) -> usize {
        self.w.nodes.iter().filter(|(_, n)| !matches!(n.place, Place::Marker(_))).count()
    }

    /// Unit `u`'s nodes in text order (for checks and traces).
    #[must_use]
    pub fn order(&self, u: UnitId) -> Vec<NodeId> {
        self.w.units.get(&u.0).map_or_else(Vec::new, |un| un.order.iter().map(|(_, &x)| NodeId(x)).collect())
    }

    #[must_use]
    pub fn live_units(&self) -> usize {
        self.units as usize
    }

    /// Frees the applications no live node outside them reads (nor a
    /// root), until none is left; the number of nodes freed.
    pub fn collect_garbage(&mut self) -> usize {
        let r = self.collect_garbage_inner();
        #[cfg(feature = "observe")]
        self.rec_call("collect_garbage", crate::record::rec().map(|_| crate::record::j(&())), &r);
        r
    }

    fn collect_garbage_inner(&mut self) -> usize {
        let roots: FxHashSet<Id> = self.roots.iter().map(|r| r.0).collect();
        let mut total = 0;
        loop {
            let dead: Vec<(u32, Id)> = self
                .w
                .units
                .iter()
                .filter(|(_, un)| un.app != super::world::NONE && self.w.node(un.app).is_some())
                .filter(|&(&u, un)| {
                    let readers = self.w.out_readers_of(un.app);
                    !readers.iter().any(|&r| roots.contains(&r) || self.w.node(r).is_some_and(|n| n.unit != u))
                })
                .map(|(&u, un)| (u, un.app))
                .collect();
            if dead.is_empty() {
                return total;
            }
            for (u, _) in dead {
                let ids: Vec<Id> = self.w.unit(u).order.iter().map(|(_, &x)| x).collect();
                let markers = ids.iter().filter(|&&x| self.w.node(x).is_some_and(|n| matches!(n.place, Place::Marker(_)))).count();
                if super::core::debug() {
                    for &x in &ids {
                        eprintln!("gc {u} {x} {}", self.debug_node(NodeId(x)).chars().take(300).collect::<String>());
                    }
                }
                total += ids.len() - markers;
                self.edit(|b| {
                    for &x in &ids {
                        if b.w.node(x).is_some_and(|n| n.parent == super::world::NONE) {
                            b.free(x);
                        }
                    }
                });
                self.w.units.remove(&u, &super::ptree::NoMeasure);
            }
        }
    }

    #[must_use]
    pub fn node_info(&self, n: NodeId) -> Option<NodeInfo> {
        let x = self.w.node(n.0)?;
        Some(NodeInfo {
            op: self.core.op_name(x.text.op),
            args: x.text.args.to_vec(),
            unit: UnitId(x.unit),
            // (A continuation is at its creator's level: no parent.)
            parent: (x.parent != super::world::NONE && matches!(x.place, Place::Child | Place::Marker(_))).then_some(NodeId(x.parent)),
            state: match x.st {
                St::Dirty | St::Held => NodeState::Dirty,
                St::Done => NodeState::Done,
                St::Inert(_) => NodeState::Inert,
            },
        })
    }

    #[must_use]
    pub fn diagnostics(&self, n: NodeId) -> &[ValueId] {
        self.w
            .node(n.0)
            .and_then(|x| x.res.as_ref())
            .map_or(&[], |r| &r.diag[..])
    }

    pub fn mark(&mut self, n: NodeId) {
        self.marked.insert(n.0);
        #[cfg(feature = "observe")]
        self.rec_call("mark", crate::record::rec().map(|_| crate::record::j(&(&n,))), &());
    }

    pub fn unmark(&mut self, n: NodeId) {
        self.marked.remove(&n.0);
        #[cfg(feature = "observe")]
        self.rec_call("unmark", crate::record::rec().map(|_| crate::record::j(&(&n,))), &());
    }

    pub fn set_version(&mut self, v: u64) {
        self.version = v;
        #[cfg(feature = "observe")]
        self.rec_call("set_version", crate::record::rec().map(|_| crate::record::j(&(&v,))), &());
    }

    pub fn take_settled(&mut self) -> Vec<Settled> {
        std::mem::take(&mut self.settled_out)
    }

    /// Re-runs the nodes whose text passes `pred`, their texts analysed
    /// again (their hooks' answers may depend on context that changed).
    pub fn refresh_where(&mut self, pred: &dyn Fn(OpId, &[ValueId], &Values) -> bool) {
        // (Recorded as the texts the predicate passed: the only texts it
        // is asked about are the cached ones and the nodes'.)
        #[cfg(feature = "observe")]
        if crate::record::rec().is_some() {
            let asked = crate::record::Asked::new(pred);
            self.refresh_where_inner(&|op, args, v| asked.ask(op, args, v));
            crate::record::api(&self.core.values, "refresh_where", asked.passed(), serde_json::Value::Null);
            return;
        }
        self.refresh_where_inner(pred);
    }

    fn refresh_where_inner(&mut self, pred: &dyn Fn(OpId, &[ValueId], &Values) -> bool) {
        // Cached analyses no node holds now go too: a text made later is
        // analysed afresh.
        for v in lock(&self.core.texts).values_mut() {
            v.retain(|t| !pred(t.op, &t.args, &self.core.values));
        }
        let mut fresh: FxHashMap<(OpId, Arc<[ValueId]>), Arc<super::world::Text>> = FxHashMap::default();
        for w in self.rounds.iter().chain(std::iter::once(&self.w)) {
            for (_, n) in w.nodes.iter() {
                if !matches!(n.place, Place::Marker(_)) && pred(n.text.op, &n.text.args, &self.core.values) {
                    fresh.entry((n.text.op, n.text.args.clone())).or_insert_with(|| self.core.reanalyze(n.text.op, n.text.args.clone()));
                }
            }
        }
        if fresh.is_empty() {
            return;
        }
        self.edit(|b| {
            let hit: Vec<(Id, Arc<super::world::Text>)> = b
                .w
                .nodes
                .iter()
                .filter_map(|(&id, n)| fresh.get(&(n.text.op, n.text.args.clone())).map(|t| (id, t.clone())))
                .collect();
            for (id, t) in hit {
                b.retext(id, t);
            }
        });
    }

    /// The sizes of the per-node records (none here: a world's nodes are
    /// persistent maps).
    #[must_use]
    pub fn record_sizes() -> (usize, usize) {
        (std::mem::size_of::<super::world::Node>(), 0)
    }

    #[must_use]
    pub fn state_dump(&self) -> String {
        format!("{} nodes, {} pending", self.w.nodes.len(), self.pending.len())
    }

    /// Committed reads that no longer hold (none in a sound state).
    #[must_use]
    pub fn check_reads(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (&id, n) in self.w.nodes.iter() {
            if n.st != St::Done {
                continue;
            }
            let Some(r) = &n.res else { continue };
            for rd in &r.reads {
                if !super::build::holds(&self.core, &self.w, id, rd) {
                    out.push(format!("node {id}: {rd:?}"));
                }
            }
        }
        out
    }

    /// Structural invariants that fail (none in a sound state).
    #[must_use]
    pub fn check_invariants(&self) -> Vec<String> {
        let mut out = Vec::new();
        for u in 0..self.units {
            let Some(un) = self.w.units.get(&u) else { continue };
            for (&p, &id) in un.order.iter() {
                match self.w.node(id) {
                    Some(n) if n.pos == p && n.unit == u => {}
                    _ => out.push(format!("unit {u}: position {p} names node {id}, which is not there")),
                }
            }
            // Each node's frame: the one a walk from the start puts it in.
            let mut stack = vec![super::world::FrameId::root(u)];
            for (_, &id) in un.order.iter() {
                let Some(n) = self.w.node(id) else { continue };
                let b = n.brackets();
                let mut k: u8 = 0;
                let mut open = |stack: &mut Vec<super::world::FrameId>, m: u32| {
                    for _ in 0..m {
                        stack.push(super::world::FrameId::opened(id, k));
                        k += 1;
                    }
                };
                let close = |stack: &mut Vec<super::world::FrameId>, m: u32| {
                    for _ in 0..m {
                        if stack.len() > 1 {
                            stack.pop();
                        }
                    }
                };
                if b.before {
                    close(&mut stack, u32::from(b.closes));
                    open(&mut stack, u32::from(b.opens));
                }
                let want = *stack.last().expect("root");
                if n.frame != want {
                    out.push(format!("unit {u}: node {id} at {} is in frame {:?}, a walk puts it in {want:?}", n.pos, n.frame));
                }
                if !b.before {
                    close(&mut stack, u32::from(b.closes));
                    open(&mut stack, u32::from(b.opens));
                }
            }
        }
        // The accumulator indexes: as the nodes in work file them.
        let mut want = super::world::World::default();
        for (&id, n) in self.w.nodes.iter() {
            want.put(id, super::world::Node::clone(n));
        }
        let list_u = |w: &World| -> Vec<(u32, Pos, Id)> { w.units.iter().flat_map(|(&u, un)| un.acc_open.iter().map(move |(&p, &x)| (u, p, x))).collect() };
        let list_f = |w: &World, res: bool| -> Vec<(u32, u64, Pos, Id)> {
            w.fields
                .iter()
                .flat_map(|(&k, c)| if res { &c.acc_res } else { &c.acc_static }.iter().map(move |(&p, &x)| (k.unit(), k.field().0.raw(), p, x)))
                .collect()
        };
        if list_u(&self.w) != list_u(&want) {
            out.push(format!("acc_open {:?}, the nodes file {:?}", list_u(&self.w), list_u(&want)));
        }
        if list_f(&self.w, false) != list_f(&want, false) {
            out.push(format!("acc_static {:?}, the nodes file {:?}", list_f(&self.w, false), list_f(&want, false)));
        }
        if list_f(&self.w, true) != list_f(&want, true) {
            out.push(format!("acc_res {:?}, the nodes file {:?}", list_f(&self.w, true), list_f(&want, true)));
        }
        let ids = |m: &super::ptree::PMap<Id, ()>| -> Vec<Id> { m.iter().map(|(&x, _)| x).collect() };
        if ids(&self.w.fins) != ids(&want.fins) {
            out.push(format!("fins {:?}, the nodes file {:?}", ids(&self.w.fins), ids(&want.fins)));
        }
        if ids(&self.w.publishers) != ids(&want.publishers) {
            out.push(format!("publishers {:?}, the nodes file {:?}", ids(&self.w.publishers), ids(&want.publishers)));
        }
        let pi = |w: &World| -> Vec<((Field, Id), ValueId)> { w.pub_ix.iter().map(|(&k, &x)| (k, x)).collect() };
        if pi(&self.w) != pi(&want) {
            out.push(format!("pub_ix {:?}, the nodes file {:?}", pi(&self.w), pi(&want)));
        }
        let fw = |w: &World| -> Vec<(u64, Id)> { w.fwds.iter().map(|(&k, _)| k).collect() };
        if fw(&self.w) != fw(&want) {
            out.push(format!("fwds {:?}, the nodes file {:?}", fw(&self.w), fw(&want)));
        }
        out
    }

    /// The last evaluation's longest chain of runs that waited for each
    /// other.
    #[must_use]
    pub fn critical_path(&self) -> u32 {
        self.chain.first().map_or(0, |c| c.1)
    }

    /// The last evaluation's critical chain, from its highest run down:
    /// each run and why it stood above the next.
    #[must_use]
    pub fn critical_chain(&self) -> Vec<(NodeId, crate::api::Cause)> {
        self.chain.iter().map(|c| (c.0, c.2.clone())).collect()
    }

    /// The realised chain, deepest first: each run, its height, and why it
    /// came after the next (as `play` prints it with `DEFUSE_RCHAIN`).
    #[must_use]
    pub fn realised_chain(&self) -> Vec<(NodeId, u64, String)> {
        self.chain.iter().map(|c| (c.0, u64::from(c.1), format!("{:?}", c.2))).collect()
    }

    /// The critical chain's runs (as [`Self::critical_chain`]) by wall
    /// time: when each was last queued, started, ended and committed, in
    /// ns since the last evaluation started (zeros on wasm32). Summed:
    /// `Report::critical_links`.
    #[must_use]
    pub fn critical_chain_times(&self) -> Vec<(NodeId, [u64; 4])> {
        self.chain_at.clone()
    }

    /// The last evaluation's chain by time, from the run committed last
    /// back: for each run, of the runs it stood on (its reads' definers and
    /// contributors, outputs, its placer, what woke it), the one committed
    /// last, which let it start; with why, and when each was queued,
    /// started, ended and committed (ns since the evaluation started). Its
    /// gaps (a start after the previous commit) are waits on none of
    /// these: a step's barrier, scheduling. ([`Self::critical_chain`] is
    /// by height: the longest chain of links, whose gaps may hide another
    /// run that let one start.)
    #[must_use]
    pub fn critical_chain_by_time(&self) -> Vec<(NodeId, crate::api::Cause, [u64; 4])> {
        self.chain_time.clone()
    }

    /// The critical chain with each run's height.
    #[must_use]
    pub fn critical_chain_heights(&self) -> Vec<(NodeId, u32, crate::api::Cause)> {
        self.chain.clone()
    }

    pub(crate) fn hook_error(&self) -> Option<crate::api::EvalError> {
        lock(&self.core.hook_error).take()
    }

}
