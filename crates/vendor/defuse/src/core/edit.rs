//! The engine: the client's handle. Text edits, declarations, and the
//! read-only queries on the committed state.

// The recording wrappers return what the call returned, unit included.
#![allow(clippy::let_unit_value, clippy::semicolon_if_nothing_returned)]

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use super::order::{NO_FRAME, Notes};
use super::{
    Data, EnvState, F_DONE, F_HAS_OUT, F_MARKED, F_NEEDS_RUN, Kind, NIL, Pred, Role, S_CLEAN, S_DIRTY, S_FREE, S_INERT,
    S_MARKER, S_NEW, SHARDS, Shared, Slot, lock, rlock, wlock,
};
use crate::Ctx;
use crate::api::{Cause, Effect, Limits, LoopStart, Monoid, NodeId, NodeInfo, OpId, Operation, Settled, Stop, UnitId};
use crate::sync::Arena;
use crate::values::{Field, FxHashMap, FxHashSet, ValueId, Values};

/// The built-in operation of `Engine::open_op`: opens a scope after it.
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

/// An incremental evaluator over units of text.
pub struct Engine {
    pub(crate) sh: Shared,
    pub(crate) roots: Vec<NodeId>,
    pub(crate) loop_start: LoopStart,
    pub(crate) max_rounds: u64,
    pub(crate) version: u64,
    pub(crate) settled_out: Vec<Settled>,
    pub(crate) last_critical: u32,
    pub(crate) heights: super::report::Heights,
    pub(crate) limits: Limits,
    pub(crate) units: Vec<u32>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Shared {
    pub(crate) fn new() -> Shared {
        Shared {
            values: Values::new(),
            ops: Vec::new(),
            projs: Vec::new(),
            op_names: Vec::new(),
            recs: Arena::new(),
            frames: Arena::new(),
            units: Arena::new(),
            fields: (0..SHARDS).map(|_| RwLock::new(FxHashMap::default())).collect(),
            by_id: (0..256).map(|_| Mutex::new(FxHashMap::default())).collect(),
            accs: FxHashMap::default(),
            seqs: FxHashSet::default(),
            loops: FxHashMap::default(),
            loop_vals: crate::sync::BrLock::new(FxHashMap::default()),
            env: crate::sync::BrLock::new(EnvState::default()),
            provider: None,
            apps: Mutex::new(FxHashMap::default()),
            env_readers: Mutex::new(FxHashMap::default()),
            loop_readers: Mutex::new(FxHashMap::default()),
            env_waits: Mutex::new(FxHashMap::default()),
            pubs: Mutex::new(FxHashMap::default()),
            pubs_dirty: Mutex::new(crate::values::FxHashSet::default()),
            undeclared: Mutex::new(FxHashMap::default()),
            q: Mutex::new(super::QueueInner::default()),
            shards: (0..super::QSHARDS)
                .map(|_| Mutex::new(std::collections::BinaryHeap::new()))
                .collect(),
            next_worker: AtomicUsize::new(0),
            work: AtomicUsize::new(0),
            qlen: AtomicUsize::new(0),
            sleepers: AtomicUsize::new(0),
            idle_mx: Mutex::new(()),
            idle_cv: std::sync::Condvar::new(),
            busy: AtomicUsize::new(0),
            stepped: AtomicBool::new(false),
            step_done: Mutex::new(Vec::new()),
            parked: Mutex::new(Vec::new()),
            touched: Mutex::new(Vec::new()),
            snaps: Mutex::new(FxHashMap::default()),
            reruns: Mutex::new(Vec::new()),
            any_preds: AtomicBool::new(false),
            any_proj: AtomicBool::new(false),
            hdirty: Mutex::new(Vec::new()),
            diags: crate::sync::Table::new(),
            diag_next: AtomicU64::new(1),
            text_memo: RwLock::new(FxHashMap::default()),
            marked: Mutex::new(Vec::new()),
            marked_ops: RwLock::new(Vec::new()),
            deferred_frames: Mutex::new(Vec::new()),
            abort: AtomicBool::new(false),
            error: Mutex::new(None),
            c: super::Counters::default(),
            t: Mutex::new(super::Tallies::default()),
            #[cfg(feature = "observe")]
            selects: Mutex::new(crate::values::FxHashMap::default()),
            epoch: AtomicU64::new(1),
            spec: AtomicU8::new(0),
            fuel: AtomicU64::new(0),
            outstanding: std::sync::atomic::AtomicUsize::new(0),
            max_outstanding: std::sync::atomic::AtomicUsize::new(0),
            limits: RwLock::new(Limits::default()),
            deadline: Mutex::new(None),
            live: AtomicU64::new(0),
            next_top: AtomicU64::new(1),
        }
    }

    /// A fresh id for a top-level node.
    fn top_id(&self) -> u64 {
        let n = self.next_top.fetch_add(1, Ordering::Relaxed);
        crate::values::hash_words(0x7079, &[n])
    }

    fn live_slot(&self, n: NodeId) -> Option<Slot> {
        self.slot_of(n.0)
            .filter(|&s| !matches!(self.rec(s).st(), S_FREE | S_MARKER))
    }

    /// Makes a top-level node and links it after `after` (`NIL`: front).
    fn make_top(&self, u: u32, after: Slot, op: OpId, args: Vec<ValueId>) -> NodeId {
        let id = self.top_id();
        let data = Data::new(Kind::Node, Role::Top, op, Arc::from(args), NIL);
        let s = self.alloc(u, id, data, 0, NIL);
        self.rec(s).set(F_NEEDS_RUN);
        let mut notes = Notes::default();
        {
            let _g = self.wunit(u);
            self.link_after(u, after, &[s]);
            self.notify_gap(s, &mut notes);
            self.register(s, &mut notes);
            notes.frames_from.push(s);
            self.settle_structure_edit(u, &mut notes);
        }
        self.dispatch(&mut notes, NIL);
        self.enqueue(s);
        NodeId(id)
    }

    /// The entry after `s`'s subtree and the continuations it made.
    fn after_all(&self, s: Slot) -> Slot {
        let mut end = self.subtree_end(s);
        loop {
            let n = self.next(end);
            if n == NIL || self.depth(n) != self.depth(s) {
                return end;
            }
            let mut o = self.rec(n).data().owner;
            let is_cont = self.rec(n).data().role == Role::Cont;
            let mut mine = false;
            let mut guard = 0;
            while is_cont && o != NIL && guard < 1 << 20 {
                if o == s {
                    mine = true;
                    break;
                }
                let d = self.rec(o).data();
                if d.role != Role::Cont {
                    break;
                }
                o = d.owner;
                guard += 1;
            }
            if !mine {
                return end;
            }
            end = self.subtree_end(n);
        }
    }
}

impl Engine {
    #[must_use]
    pub fn new() -> Engine {
        let mut eng = Engine {
            sh: Shared::new(),
            roots: Vec::new(),
            loop_start: LoopStart::Initial,
            max_rounds: 64,
            version: 0,
            settled_out: Vec::new(),
            last_critical: 0,
            heights: super::report::Heights::default(),
            limits: Limits::default(),
            units: Vec::new(),
        };
        eng.register(Arc::new(OpenOp));
        eng.register(Arc::new(CloseOp));
        eng
    }

    #[must_use]
    pub fn values(&self) -> &Values {
        &self.sh.values
    }

    pub fn register(&mut self, op: Arc<dyn Operation>) -> OpId {
        #[cfg(feature = "observe")]
        #[allow(clippy::cast_possible_truncation)]
        // The two built-in operations are the engine's own: not recorded.
        let op: Arc<dyn Operation> = if crate::record::rec().is_some() && self.sh.ops.len() >= 2 {
            let id = OpId(self.sh.ops.len() as u32);
            crate::record::api(
                &self.sh.values,
                "register",
                serde_json::json!([op.name()]),
                crate::record::j(&id),
            );
            Arc::new(crate::record::RecOp { inner: op, id })
        } else {
            op
        };
        let name = op.name();
        self.sh.ops.push(op);
        self.sh.op_names.push(name);
        #[allow(clippy::cast_possible_truncation)]
        OpId((self.sh.ops.len() - 1) as u32)
    }

    /// Registers a projection for [`Ctx::use_field_proj`]. It must be pure:
    /// the engine applies it again to decide whether a changed value still
    /// projects equal.
    ///
    /// # Panics
    ///
    /// If more than 65,533 projections are registered.
    pub fn register_projection(&mut self, p: crate::api::Projection) -> crate::api::ProjId {
        #[cfg(feature = "observe")]
        #[allow(clippy::cast_possible_truncation)]
        let p = {
            let id = (self.sh.projs.len() + 2) as u16;
            crate::record::api(
                &self.sh.values,
                "register_projection",
                serde_json::Value::Null,
                serde_json::json!(id),
            );
            crate::record::rec_projection(id, p)
        };
        self.sh.projs.push(p);
        crate::api::ProjId(u16::try_from(self.sh.projs.len() + 1).expect("too many projections"))
    }

    /// The built-in operation that opens a scope after its node.
    #[must_use]
    pub fn open_op(&self) -> OpId {
        OpId(0)
    }

    pub fn new_unit(&mut self) -> UnitId {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.new_unit_inner();
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "new_unit", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn new_unit_inner(&mut self) -> UnitId {
        let u = self.sh.new_unit_rec(NIL);
        self.units.push(u);
        UnitId(u)
    }

    /// Appends a node at the end of `unit`.
    pub fn push(&mut self, unit: UnitId, op: OpId, args: Vec<ValueId>) -> NodeId {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&unit, &op, &args)) } else { serde_json::Value::Null };
        let r = self.push_inner(unit, op, args);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "push", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn push_inner(&mut self, unit: UnitId, op: OpId, args: Vec<ValueId>) -> NodeId {
        let last = self.sh.unit(unit.0).last.load(Ordering::Acquire);
        self.sh.make_top(unit.0, last, op, args)
    }

    /// Appends `n` nodes to the end of `unit`, in order: the same result
    /// as `n` calls of [`Engine::push`].
    pub fn push_many(&mut self, unit: UnitId, op: OpId, args: Vec<Vec<ValueId>>) -> Vec<NodeId> {
        args.into_iter().map(|a| self.push(unit, op, a)).collect()
    }

    /// Keeps the node's applications alive across collections.
    pub fn add_root(&mut self, n: NodeId) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&n,)) } else { serde_json::Value::Null };
        let r = self.add_root_inner(n);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "add_root", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn add_root_inner(&mut self, n: NodeId) {
        self.roots.push(n);
    }

    /// # Panics
    ///
    /// If `node` is not a live node.
    pub fn insert_after(&mut self, node: NodeId, op: OpId, args: Vec<ValueId>) -> NodeId {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&node, &op, &args)) } else { serde_json::Value::Null };
        let r = self.insert_after_inner(node, op, args);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "insert_after", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn insert_after_inner(&mut self, node: NodeId, op: OpId, args: Vec<ValueId>) -> NodeId {
        let s = self.sh.live_slot(node).expect("insert_after: a live node");
        let u = self.sh.unit_of(s);
        let after = {
            let _g = rlock(&self.sh.unit(u).s);
            self.sh.after_all(s)
        };
        self.sh.make_top(u, after, op, args)
    }

    /// # Panics
    ///
    /// If `node` is not a live node.
    pub fn insert_before(&mut self, node: NodeId, op: OpId, args: Vec<ValueId>) -> NodeId {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&node, &op, &args)) } else { serde_json::Value::Null };
        let r = self.insert_before_inner(node, op, args);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "insert_before", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn insert_before_inner(&mut self, node: NodeId, op: OpId, args: Vec<ValueId>) -> NodeId {
        let s = self.sh.live_slot(node).expect("insert_before: a live node");
        let u = self.sh.unit_of(s);
        let p = self.sh.prev(s);
        self.sh.make_top(u, p, op, args)
    }

    /// Replaces a node's text; the node keeps its id.
    pub fn replace(&mut self, node: NodeId, op: OpId, args: Vec<ValueId>) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&node, &op, &args)) } else { serde_json::Value::Null };
        let r = self.replace_inner(node, op, args);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "replace", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn replace_inner(&mut self, node: NodeId, op: OpId, args: Vec<ValueId>) {
        let Some(s) = self.sh.live_slot(node) else { return };
        let sh = &self.sh;
        let r = sh.rec(s);
        let u = sh.unit_of(s);
        let mut notes = Notes::default();
        {
            let _g = sh.wunit(u);
            sh.snapshot(s);
            sh.unregister_named(s);
            sh.leave_work_lists(s);
            let old_static_claim = r.data().stat.claim;
            {
                let mut d = r.data();
                d.op = op;
                d.args = Arc::from(args);
                d.thash = super::text_hash_of(d.op, &d.args);
                d.kept = None;
            }
            if old_static_claim != super::ClaimS::None {
                let claimed = r.data().take_claimed();
                for x in claimed {
                    sh.release(x, s, &mut notes);
                }
            }
            let st = r.st();
            if st == S_INERT {
                // Claimed: only the claimer sees the new text.
                sh.analyze_into(s);
            } else {
                // Declared definitions the new text no longer declares.
                let old_decl = r.data().stat.defs.clone();
                sh.register(s, &mut notes);
                let new_decl = r.data().stat.defs.clone();
                if let Some(od) = old_decl {
                    let defined: Vec<Field> = sh.defs_of(s).iter().map(|d| d.f).collect();
                    for (f, _) in od {
                        let still = new_decl.as_ref().is_some_and(|nd| nd.iter().any(|e| e.0 == f));
                        if !still && !defined.contains(&f) {
                            sh.remove_def(u, s, f, &mut notes);
                        }
                    }
                }
                r.set(F_NEEDS_RUN);
                {
                    let mut d = r.data();
                    sh.note_rerun(s, crate::api::Rerun::Text);
                    if d.causes.len() < 8 {
                        d.causes.push(Cause::Structure);
                    }
                }
                if matches!(st, S_CLEAN | S_DIRTY | super::S_PARK | super::S_STEP) {
                    r.status.store(S_DIRTY, Ordering::Release);
                }
                notes.enqueue.push(s);
            }
            sh.notify_sib(s, &mut notes);
            sh.notify_gap(s, &mut notes);
            notes.frames_from.push(s);
            sh.settle_structure_edit(u, &mut notes);
        }
        sh.dispatch(&mut notes, NIL);
        sh.enter_work_lists(s);
    }

    /// Removes a node and its subtree.
    pub fn remove(&mut self, node: NodeId) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&node,)) } else { serde_json::Value::Null };
        let r = self.remove_inner(node);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "remove", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn remove_inner(&mut self, node: NodeId) {
        let Some(s) = self.sh.live_slot(node) else { return };
        let sh = &self.sh;
        let u = sh.unit_of(s);
        let mut notes = Notes::default();
        {
            let _g = sh.wunit(u);
            sh.free_entry(s, &mut notes);
            sh.settle_structure_edit(u, &mut notes);
        }
        sh.dispatch(&mut notes, NIL);
        self.roots.retain(|&r| r != node);
    }

    /// The committed output of a node (`None`: not run, claimed or gone).
    #[must_use]
    pub fn output(&self, n: NodeId) -> Option<ValueId> {
        let s = self.sh.live_slot(n)?;
        let r = self.sh.rec(s);
        if r.st() == S_INERT || !r.has(F_HAS_OUT) {
            return None;
        }
        Some(ValueId::from_raw(r.out.load(Ordering::Acquire)))
    }

    /// A node's children (scope markers left out).
    #[must_use]
    pub fn children(&self, n: NodeId) -> Vec<NodeId> {
        let Some(s) = self.sh.live_slot(n) else {
            return Vec::new();
        };
        let kids: Vec<Slot> = {
            let d = self.sh.rec(s).data();
            d.kids.iter().chain(&d.conts).copied().collect()
        };
        kids.into_iter()
            .filter(|&k| !self.sh.is_marker(k))
            .map(|k| self.sh.node_id(k))
            .collect()
    }

    /// The siblings a node claims.
    #[must_use]
    pub fn claimed(&self, n: NodeId) -> Vec<NodeId> {
        let Some(s) = self.sh.live_slot(n) else {
            return Vec::new();
        };
        let c = self.sh.rec(s).data().claimed();
        c.into_iter().map(|k| self.sh.node_id(k)).collect()
    }

    /// The node whose definition of `f` reaches `n`.
    #[must_use]
    pub fn reaching_def(&self, n: NodeId, f: Field) -> Option<NodeId> {
        let s = self.sh.live_slot(n)?;
        let _g = rlock(&self.sh.unit(self.sh.unit_of(s)).s);
        match self.sh.q_def(s, f, false, s) {
            super::query::Q::Ans(o) => match o.ans {
                super::DAns::Site { node, .. } => Some(self.sh.node_id(node)),
                _ => None,
            },
            super::query::Q::Wait(_) => None,
        }
    }

    /// The value `n` defines for `f`.
    #[must_use]
    pub fn def_value(&self, n: NodeId, f: Field) -> Option<ValueId> {
        let s = self.sh.live_slot(n)?;
        // The field's index has the node's entry: no walk over all of the
        // node's definitions (a prelude node makes thousands).
        let u = self.sh.unit_of(s);
        let sh = rlock(self.sh.shard(u, f));
        let idx = sh.get(&(u, f))?;
        let i = self.sh.def_pos(idx, s)?;
        match idx.defs[i].val {
            super::DefVal::Val(x) | super::DefVal::Push(x) | super::DefVal::Repl(x) => Some(x),
            _ => None,
        }
    }

    /// The fold of accumulator `f` over `unit`.
    #[must_use]
    pub fn acc_value(&self, unit: UnitId, f: Field) -> Option<ValueId> {
        self.sh.acc_total(unit.0, f)
    }

    pub fn declare_accumulator(&mut self, f: Field, m: Arc<dyn Monoid>) {
        #[cfg(feature = "observe")]
        let m: Arc<dyn Monoid> = {
            crate::record::api(
                &self.sh.values,
                "declare_accumulator",
                crate::record::j(&f),
                serde_json::Value::Null,
            );
            Arc::new(crate::record::RecMonoid { inner: m, field: f })
        };
        self.sh.accs.insert(f, m);
    }

    pub fn declare_sequence(&mut self, f: Field) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&f,)) } else { serde_json::Value::Null };
        let r = self.declare_sequence_inner(f);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "declare_sequence", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn declare_sequence_inner(&mut self, f: Field) {
        self.sh.seqs.insert(f);
    }

    pub fn declare_loopback(&mut self, f: Field, init: ValueId) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&f, &init)) } else { serde_json::Value::Null };
        let r = self.declare_loopback_inner(f, init);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "declare_loopback", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn declare_loopback_inner(&mut self, f: Field, init: ValueId) {
        if let Some(&old) = self.sh.loops.get(&f) {
            // Declared again: the same is nothing; a new initial value is
            // the round value only while the field still has the old one.
            if old == init {
                return;
            }
            self.sh.loops.insert(f, init);
            let moved = {
                let mut lv = self.sh.loop_vals.write();
                let at_init = lv.get(&f) == Some(&old);
                if at_init {
                    lv.insert(f, init);
                }
                at_init
            };
            if moved {
                let readers = lock(&self.sh.loop_readers).remove(&f).unwrap_or_default();
                for (n, g) in readers {
                    self.sh.touch(n, g, &Cause::Loop { field: f }, false);
                }
            }
            return;
        }
        self.sh.loops.insert(f, init);
        self.sh.loop_vals.write().insert(f, init);
        lock(&self.sh.pubs_dirty).insert(f);
        // Readers that bound `f` otherwise read the round value now: those
        // that found a definition, and those that fell through to the
        // environment (nothing defined it).
        let mut notes = Notes::default();
        self.touch_field_readers(f, &mut notes);
        let env = lock(&self.sh.env_readers).remove(&f).unwrap_or_default();
        for (n, g) in env {
            self.sh.touch(n, g, &Cause::Loop { field: f }, false);
        }
        self.sh.dispatch(&mut notes, NIL);
    }

    fn touch_field_readers(&self, f: Field, notes: &mut Notes) {
        for &u in &self.all_units() {
            let sh = rlock(self.sh.shard(u, f));
            if let Some(idx) = sh.get(&(u, f)) {
                for sub in &idx.readers {
                    notes.touch.push((sub.node, sub.gn, Cause::Loop { field: f }));
                }
            }
        }
    }

    fn all_units(&self) -> Vec<u32> {
        #[allow(clippy::cast_possible_truncation)]
        (0..self.sh.units.len() as u32)
            .filter(|&u| self.sh.unit(u).live.load(Ordering::Acquire))
            .collect()
    }

    pub fn set_loop_start(&mut self, s: LoopStart) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&s,)) } else { serde_json::Value::Null };
        let r = self.set_loop_start_inner(s);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "set_loop_start", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn set_loop_start_inner(&mut self, s: LoopStart) {
        self.loop_start = s;
    }

    pub fn set_max_rounds(&mut self, n: u64) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&n,)) } else { serde_json::Value::Null };
        let r = self.set_max_rounds_inner(n);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "set_max_rounds", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn set_max_rounds_inner(&mut self, n: u64) {
        self.max_rounds = n;
    }

    /// Bounds an evaluation (see [`Limits`]); exceeding one is an error.
    pub fn set_limits(&mut self, l: Limits) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&l,)) } else { serde_json::Value::Null };
        let r = self.set_limits_inner(l);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "set_limits", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn set_limits_inner(&mut self, l: Limits) {
        *wlock(&self.sh.limits) = l.clone();
        self.limits = l;
    }

    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Supplies environment fields (replacing the previous ones).
    pub fn set_environment(&mut self, env: impl IntoIterator<Item = (Field, ValueId)>) {
        let new: FxHashMap<Field, ValueId> = env.into_iter().collect();
        #[cfg(feature = "observe")]
        {
            let mut v: Vec<(Field, ValueId)> = new.iter().map(|(&f, &x)| (f, x)).collect();
            v.sort_unstable();
            crate::record::api(
                &self.sh.values,
                "set_environment",
                crate::record::j(&v),
                serde_json::Value::Null,
            );
        }
        // By value: only the fields whose answer moves wake their readers
        // (the provider's answers stay underneath).
        let changed = {
            let mut e = self.sh.env.write();
            let mut fs: Vec<Field> = e.known.keys().chain(new.keys()).copied().collect();
            fs.sort_unstable_by_key(|f| f.0.raw());
            fs.dedup();
            let before: Vec<_> = fs.iter().map(|&f| e.get(f)).collect();
            e.known = new;
            fs.into_iter()
                .zip(before)
                .filter(|(f, b)| e.get(*f) != *b)
                .map(|(f, _)| f)
                .collect::<Vec<_>>()
        };
        self.env_changed(&changed);
    }

    /// Sets the value a field has at the start of `unit`.
    pub fn set_env(&mut self, unit: UnitId, field: Field, value: ValueId) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&unit, &field, &value)) } else { serde_json::Value::Null };
        let r = self.set_env_inner(unit, field, value);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "set_env", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn set_env_inner(&mut self, unit: UnitId, field: Field, value: ValueId) {
        let old = self.sh.unit(unit.0).env.write().insert(field, value);
        if old != Some(value) {
            self.env_changed(&[field]);
        }
    }

    /// Keeps `v` alive across value sweeps (for values the client holds).
    /// (Values are never swept here: every id stays valid.)
    pub fn pin(&mut self, v: ValueId) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&v,)) } else { serde_json::Value::Null };
        let r = self.pin_inner(v);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "pin", a, serde_json::Value::Null);
        r
    }

    #[inline]
    #[allow(clippy::unused_self)]
    fn pin_inner(&mut self, v: ValueId) {
        let _ = v;
    }

    /// Undoes one [`Self::pin`].
    pub fn unpin(&mut self, v: ValueId) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&v,)) } else { serde_json::Value::Null };
        let r = self.unpin_inner(v);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "unpin", a, serde_json::Value::Null);
        r
    }

    #[inline]
    #[allow(clippy::unused_self)]
    fn unpin_inner(&mut self, v: ValueId) {
        let _ = v;
    }

    /// The values' epoch: the number of sweeps so far. A [`ValueId`] made
    /// or read at one epoch stays valid while the epoch is unchanged, so
    /// a client cache keyed by the epoch is valid by construction;
    /// [`Ctx::values_epoch`] gives it to a run. (No sweeps here: always 0.)
    #[must_use]
    pub fn values_epoch(&self) -> u64 {
        0
    }

    /// Removes the environment provider.
    pub fn clear_env_provider(&mut self) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.clear_env_provider_inner();
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "clear_env_provider", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn clear_env_provider_inner(&mut self) {
        self.sh.provider = None;
    }

    /// Undoes [`Self::add_root`].
    pub fn remove_root(&mut self, node: NodeId) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&node,)) } else { serde_json::Value::Null };
        let r = self.remove_root_inner(node);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "remove_root", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn remove_root_inner(&mut self, node: NodeId) {
        self.roots.retain(|&r| r != node);
    }

    /// Whether `node` is a node of the program now.
    #[must_use]
    pub fn is_live(&self, node: NodeId) -> bool {
        self.sh.live_slot(node).is_some()
    }

    /// A node's committed reads of definitions, one per line: field, the
    /// node whose definition answered (or env / loop), the value, and the
    /// node a speculative read guessed from. For tracking down a wrong
    /// result after an edit.
    #[must_use]
    pub fn debug_reads(&self, id: NodeId) -> String {
        let Some(s) = self.sh.live_slot(id) else {
            return format!("{id:?}: gone");
        };
        let sh = &self.sh;
        let log = sh.rec(s).data().res.as_ref().map(|c| c.log.clone()).unwrap_or_default();
        let mut out = String::new();
        for p in &log {
            if let super::Pred::Def { f, ans, spec, mode, .. } = p {
                let name = sh.values.render(f.0);
                let (from, val) = match ans {
                    super::DAns::Site { node, val, .. } => (
                        format!("{:?}", sh.node_id(*node)),
                        match val {
                            super::DefVal::Val(x) | super::DefVal::Push(x) | super::DefVal::Repl(x) => sh.values.render(*x),
                            other => format!("{other:?}"),
                        },
                    ),
                    super::DAns::Env(x) => ("env".into(), x.map_or("none".into(), |x| sh.values.render(x))),
                    super::DAns::Loop(x) => ("loop".into(), sh.values.render(*x)),
                    super::DAns::Seq(x) => ("seq".into(), x.map_or("none".into(), |x| sh.values.render(x))),
                };
                let guess = spec.map_or(String::new(), |g| format!(" guessed from {:?}", sh.node_id(g)));
                out.push_str(&format!("{name} ({mode:?}): from {from} = {val}{guess}\n"));
            }
        }
        out
    }

    /// A node's scheduling state, for diagnostics.
    #[must_use]
    pub fn debug_node(&self, id: NodeId) -> String {
        let Some(s) = self.sh.live_slot(id) else {
            return format!("{id:?}: gone");
        };
        let r = self.sh.rec(s);
        let d = r.data();
        format!(
            "{id:?}: {} {}({}) flags {:#x} label {} frame {} waits on {:?}",
            Shared::status_name(r.st()),
            self.sh.op_name(d.op),
            d.args
                .iter()
                .map(|a| self.sh.values.render(*a))
                .collect::<Vec<_>>()
                .join(" "),
            r.fl(),
            r.lab(),
            self.sh.frame_of(s),
            d.wait_on().map(|w| (w.on.clone(), w.kind))
        )
    }

    /// Marks every node created with `op` from now on (the client's
    /// output nodes, say), at creation.
    pub fn mark_op(&mut self, op: OpId) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&op,)) } else { serde_json::Value::Null };
        let r = self.mark_op_inner(op);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "mark_op", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn mark_op_inner(&mut self, op: OpId) {
        let mut m = wlock(&self.sh.marked_ops);
        if !m.contains(&op) {
            m.push(op);
        }
    }

    /// The built-in operation that closes a scope.
    #[must_use]
    pub fn close_op(&self) -> OpId {
        OpId(1)
    }

    /// Declares environment fields absent (replacing the previous ones).
    pub fn set_environment_absent(&mut self, absent: impl IntoIterator<Item = Field>) {
        let new: FxHashSet<Field> = absent.into_iter().collect();
        #[cfg(feature = "observe")]
        {
            let mut v: Vec<Field> = new.iter().copied().collect();
            v.sort_unstable();
            crate::record::api(
                &self.sh.values,
                "set_environment_absent",
                crate::record::j(&v),
                serde_json::Value::Null,
            );
        }
        let changed = {
            let mut e = self.sh.env.write();
            let mut fs: Vec<Field> = e.absent.iter().chain(new.iter()).copied().collect();
            fs.sort_unstable_by_key(|f| f.0.raw());
            fs.dedup();
            let before: Vec<_> = fs.iter().map(|&f| e.get(f)).collect();
            e.absent = new;
            fs.into_iter()
                .zip(before)
                .filter(|(f, b)| e.get(*f) != *b)
                .map(|(f, _)| f)
                .collect::<Vec<_>>()
        };
        self.env_changed(&changed);
    }

    fn env_changed(&mut self, fields: &[Field]) {
        let sh = &self.sh;
        for &f in fields {
            let readers = lock(&sh.env_readers).remove(&f).unwrap_or_default();
            for (n, g) in readers {
                sh.touch(n, g, &Cause::Environment { field: f }, false);
            }
            let waits = lock(&sh.env_waits).remove(&f).unwrap_or_default();
            for (n, q) in waits {
                sh.wake(n, q);
            }
        }
    }

    /// The supplied environment.
    pub fn environment(&self) -> impl Iterator<Item = (Field, ValueId)> + '_ {
        let e = self.sh.env.read();
        let mut v: Vec<(Field, ValueId)> = e.known.iter().map(|(f, x)| (*f, *x)).collect();
        v.extend(
            e.provided
                .iter()
                .filter(|(f, _)| !e.known.contains_key(*f) && !e.absent.contains(*f))
                .filter_map(|(f, x)| x.map(|x| (*f, x))),
        );
        drop(e);
        v.into_iter()
    }

    /// Supplies environment fields on demand: `Some(answer)` known
    /// (`Some(None)`: absent), `None` unknown.
    pub fn set_env_provider(&mut self, provider: Box<crate::api::EnvProvider>) {
        #[cfg(feature = "observe")]
        let provider = {
            crate::record::api(
                &self.sh.values,
                "set_env_provider",
                serde_json::Value::Null,
                serde_json::Value::Null,
            );
            crate::record::rec_provider(provider)
        };
        self.sh.provider = Some(provider);
    }

    #[must_use]
    pub fn live_nodes(&self) -> usize {
        usize::try_from(self.sh.live.load(Ordering::Relaxed)).unwrap_or(usize::MAX)
    }

    #[must_use]
    pub fn live_units(&self) -> usize {
        self.all_units().len()
    }

    /// Frees applications no live node reads (and their bodies); returns
    /// the nodes freed.
    pub fn collect_garbage(&mut self) -> usize {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.collect_garbage_inner();
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "collect_garbage", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn collect_garbage_inner(&mut self) -> usize {
        self.collect_apps(false)
    }

    /// Frees the unreferenced applications (only those this evaluation
    /// made, with `fresh`: a refuted guess's).
    pub(crate) fn collect_apps(&mut self, fresh: bool) -> usize {
        let before = self.sh.live.load(Ordering::Relaxed);
        let ep = self.sh.epoch.load(Ordering::Relaxed);
        loop {
            let apps: Vec<Slot> = lock(&self.sh.apps).values().copied().collect();
            let mut freed_any = false;
            for a in apps {
                if self.sh.rec(a).st() == S_FREE || (fresh && self.sh.rec(a).data().born < ep) || self.app_referenced(a)
                {
                    continue;
                }
                let u = self.sh.unit_of(a);
                let au = self.sh.rec(a).data().app_unit();
                let mut notes = Notes::default();
                {
                    let _g = self.sh.wunit(u);
                    self.sh.free_entry(a, &mut notes);
                    self.sh.settle_structure(u, &mut notes);
                }
                if au != u32::MAX {
                    self.sh.unit(au).live.store(false, Ordering::Release);
                }
                self.sh.dispatch(&mut notes, NIL);
                freed_any = true;
            }
            if !freed_any {
                break;
            }
        }
        let after = self.sh.live.load(Ordering::Relaxed);
        usize::try_from(before.saturating_sub(after)).unwrap_or(0)
    }

    pub(crate) fn app_referenced(&self, a: Slot) -> bool {
        let users = self.sh.rec(a).data().out_users.clone();
        let au = self.sh.rec(a).data().app_unit();
        users.iter().any(|&(n, g)| {
            let r = self.sh.rec(n);
            r.st() != S_FREE
                && r.lgen.load(Ordering::Acquire) == g
                && self.sh.unit_of(n) != au
                && r.data()
                    .res
                    .as_ref()
                    .is_some_and(|c| c.log.iter().any(|p| matches!(p, Pred::App { node, .. } if *node == a)))
        })
    }

    #[must_use]
    pub fn node_info(&self, n: NodeId) -> Option<NodeInfo> {
        let s = self.sh.live_slot(n)?;
        let r = self.sh.rec(s);
        let p = r.parent.load(Ordering::Acquire);
        let d = r.data();
        Some(NodeInfo {
            op: self.sh.op_name(d.op),
            args: d.args.to_vec(),
            parent: (p != NIL).then(|| self.sh.node_id(p)),
            unit: UnitId(self.sh.unit_of(s)),
            state: match r.st() {
                S_INERT => crate::NodeState::Inert,
                super::S_PARK => crate::NodeState::Parked,
                S_CLEAN | super::S_STEP if r.has(F_DONE) => crate::NodeState::Done,
                _ => crate::NodeState::Dirty,
            },
        })
    }

    /// The diagnostics the node's last committed run gave.
    #[must_use]
    pub fn diagnostics(&self, n: NodeId) -> &[ValueId] {
        let Some(s) = self.sh.live_slot(n) else {
            return &[];
        };
        let r = self.sh.rec(s);
        if r.st() != S_CLEAN && r.st() != super::S_STEP {
            return &[];
        }
        let k = r.data().diag;
        if k == 0 {
            return &[];
        }
        self.sh.diags.get(k).map_or(&[], |d| &**d)
    }

    /// Asks for a report when the node is final.
    pub fn mark(&mut self, n: NodeId) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&n,)) } else { serde_json::Value::Null };
        let r = self.mark_inner(n);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "mark", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn mark_inner(&mut self, n: NodeId) {
        if let Some(s) = self.sh.live_slot(n) {
            if !self.sh.rec(s).has(F_MARKED) {
                lock(&self.sh.marked).push(s);
            }
            self.sh.rec(s).set(F_MARKED);
            if let Some(e) = self.sh.rec(s).data().x.as_mut() {
                e.reported = None;
            }
        }
    }

    pub fn unmark(&mut self, n: NodeId) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&n,)) } else { serde_json::Value::Null };
        let r = self.unmark_inner(n);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "unmark", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn unmark_inner(&mut self, n: NodeId) {
        if let Some(s) = self.sh.live_slot(n) {
            self.sh.rec(s).clear(F_MARKED);
        }
    }

    /// The client's version of the text from now on.
    pub fn set_version(&mut self, v: u64) {
        #[cfg(feature = "observe")]
        let a = if crate::record::rec().is_some() { crate::record::j(&(&v,)) } else { serde_json::Value::Null };
        let r = self.set_version_inner(v);
        #[cfg(feature = "observe")]
        crate::record::api(&self.sh.values, "set_version", a, serde_json::Value::Null);
        r
    }

    #[inline]
    fn set_version_inner(&mut self, v: u64) {
        self.version = v;
    }

    /// Settled reports no edit source took.
    pub fn take_settled(&mut self) -> Vec<Settled> {
        std::mem::take(&mut self.settled_out)
    }

    /// Re-runs the nodes whose text passes `pred`.
    pub fn refresh_where(&mut self, pred: &dyn Fn(OpId, &[ValueId], &Values) -> bool) {
        let sh = &self.sh;
        for u in self.all_units() {
            let entries = {
                let _g = rlock(&sh.unit(u).s);
                sh.entries(u)
            };
            for s in entries {
                let r = sh.rec(s);
                if matches!(r.st(), S_FREE | S_MARKER | S_INERT) {
                    continue;
                }
                let (op, args) = sh.text(s);
                if pred(op, &args, &sh.values) {
                    // Its static facts may have changed with the context.
                    let mut notes = Notes::default();
                    {
                        let _g = sh.wunit(u);
                        sh.unregister_named(s);
                        sh.leave_work_lists(s);
                        sh.register(s, &mut notes);
                        if !super::in_work(r.st()) {
                            sh.leave_work_lists(s);
                        }
                        notes.frames_from.push(s);
                        sh.settle_structure(u, &mut notes);
                    }
                    sh.dispatch(&mut notes, NIL);
                    r.set(F_NEEDS_RUN);
                    sh.touch(s, u32::MAX, &Cause::Structure, true);
                }
            }
        }
    }

    /// Sizes of a node record and a use edge, in bytes.
    #[must_use]
    pub fn record_sizes() -> (usize, usize) {
        (size_of::<super::Rec>(), size_of::<super::Sub>())
    }

    /// The scheduler's state as text: counters, workers, the ready queue,
    /// node counts by state, and every node in work with its operation,
    /// arguments, unit, depth, state and what it waits on (parked ones
    /// first). Safe to call from another thread during an evaluation.
    #[must_use]
    pub fn state_dump(&self) -> String {
        self.sh.dump()
    }

    /// Consistency checks of the committed state; empty when it holds.
    #[must_use]
    pub fn check_invariants(&self) -> Vec<String> {
        let sh = &self.sh;
        sh.settle_deferred();
        let mut out = Vec::new();
        for shard in &*sh.fields {
            for (&(u, f), idx) in rlock(shard).iter() {
                for e in &idx.defs {
                    if sh.rec(e.node).st() != S_FREE && e.fk != sh.frame_of(e.node) {
                        out.push(format!(
                            "unit {u}: {f:?} entry of {} filed under frame {} not {}",
                            e.node,
                            e.fk,
                            sh.frame_of(e.node)
                        ));
                    }
                }
                let filed: usize = idx.local.values().map(Vec::len).sum();
                if filed != idx.defs.len() {
                    out.push(format!(
                        "unit {u}: {f:?} has {} entries, {filed} filed by frame",
                        idx.defs.len()
                    ));
                }
            }
        }
        for u in self.all_units() {
            let es = sh.entries(u);
            let mut last = 0u64;
            let mut prev = NIL;
            for &s in &es {
                let r = sh.rec(s);
                let l = r.lab();
                if prev != NIL && l <= last {
                    out.push(format!("unit {u}: labels out of order at {s}"));
                }
                if sh.prev(s) != prev {
                    out.push(format!("unit {u}: broken back link at {s}"));
                }
                last = l;
                prev = s;
                let st = r.st();
                if st == S_FREE {
                    out.push(format!("unit {u}: free entry {s} in the order"));
                }
                if super::in_work(st) {
                    let parked_env = r.data().kept.is_some() || st == super::S_PARK;
                    if !parked_env {
                        out.push(format!(
                            "unit {u}: node {s} still in work ({})",
                            Shared::status_name(st)
                        ));
                    }
                }
                if st == S_CLEAN && !r.has(F_DONE) {
                    out.push(format!("unit {u}: clean node {s} without a result"));
                }
                if st == S_CLEAN {
                    let log = r.data().res.as_ref().map_or_else(Default::default, |c| c.log.clone());
                    super::query::sib_cursor_reset();
                    for p in &log {
                        if !matches!(sh.recheck(s, p, false), Ok(Ok(()))) {
                            let (op, a) = sh.text(s);
                            let a: Vec<String> = a.iter().map(|x| sh.values.render(*x)).collect();
                            out.push(format!(
                                "unit {u}: clean node {s} {}({}) has a broken read {p:?}",
                                sh.op_name(op),
                                a.join(", ")
                            ));
                            break;
                        }
                    }
                }
                if sh.frame_of(s) == NO_FRAME {
                    out.push(format!("unit {u}: entry {s} without a frame"));
                }
            }
        }
        let _ = S_NEW;
        out
    }
}

impl Shared {
    /// Static facts of an inert node's new text (nothing registered).
    pub(crate) fn analyze_into(&self, s: Slot) {
        let st = self.analyze(s);
        self.rec(s).data().stat = st;
    }
}
