//! A wide batch applied in slices, each through an overlay engine.
//!
//! The batch's results, in text order, are cut into slices of consecutive
//! results of one unit (`apply_results`). Each slice is applied by an
//! overlay: an `Engine` whose structures hold only what the slice wrote
//! and read everything else from the engine as it was before the batch
//! (the base, see `par`). A slice writes only the nodes in its label range;
//! a write to any other node goes to its tail, replayed after the merge.
//! The overlays are then merged into the engine in slice order, the tails
//! drained, and the results that read or change more than their
//! neighbourhood applied whole. Each slice sees the batch-start state of
//! everything outside it, and every write outside it lands after all of
//! them: the outcome is that of a serial order of the batch.
//!
//! A slice that meets something an overlay does not support (freeing a
//! node, renumbering labels, changing a scope boundary, ...) aborts: what
//! it wrote is dropped and its results are applied whole.

use super::{Aux, Engine, FxHashMap, NO_AUX, Node, OItem, OrderOp, Report, Rerun, RunResult, Slot, State, TNIL, Unit};
use crate::ix::{LabelIx, SlotCounts, SlotMap};
use crate::obs::{count, span};
use crate::ov::{OvMap, OvSet};
use crate::par::Base;
use crate::sched::Executor;
use crate::tree::Arena;
use crate::value::{Field, ValueId};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, PoisonError, RwLock};

/// Aux ids an overlay made (its own `aux`, not the base's).
pub(super) const AUX_LOCAL: u32 = 1 << 31;
/// A stage entry: the slice id above, the overlay's copy below.
const SID_SHIFT: u32 = 16;
const LOCAL_MASK: u32 = (1 << SID_SHIFT) - 1;

/// What an overlay holds beyond the engine fields it shares the type of.
pub(super) struct Ov {
    pub(super) base: Base<Engine>,
    /// This slice's id (from 1): its copies are marked with it in the
    /// base's `stage`.
    pub(super) sid: u32,
    /// Copies of the nodes the slice wrote, and their slots.
    pub(super) nodes: Vec<Node>,
    pub(super) slots: Vec<Slot>,
    /// The auxes it wrote or made, with where each came from (`NO_AUX`:
    /// made here) and whether its kept run state was set.
    pub(super) aux: Vec<Aux>,
    pub(super) aux_home: Vec<u32>,
    pub(super) susp_set: Vec<bool>,
    /// Slots reserved for the nodes it creates (taken from the end).
    pub(super) fresh: Vec<Slot>,
    pub(super) roots: FxHashMap<u32, u32>,
    pub(super) acc_roots: FxHashMap<(u32, Field), u32>,
    pub(super) order_log: Vec<OrderOp>,
    pub(super) acc_log: Vec<(u32, Field, u64, Option<ValueId>)>,
    pub(super) done_log: Vec<(u32, bool)>,
    pub(super) abort: Option<&'static str>,
    /// Where writes go once aborted.
    pub(super) scratch: Node,
    pub(super) scratch_aux: Aux,
    pub(super) scratch_unit: Unit,
    pub(super) pending_p_lens0: BTreeMap<usize, u32>,
    pub(super) spec_live0: u32,
    pub(super) tracing0: u32,
}

impl Ov {
    #[inline]
    fn base(&self) -> &Engine {
        self.base.get().expect("an overlay has a base")
    }

    /// The overlay's copy index of slot `ix`, if it has one.
    #[inline]
    fn copy_of(&self, b: &Engine, ix: usize) -> Option<usize> {
        let t = b.stage.get(ix)?.load(Ordering::Relaxed);
        (t >> SID_SHIFT == self.sid).then_some((t & LOCAL_MASK) as usize)
    }

    #[inline]
    pub(super) fn node(&self, s: Slot) -> &Node {
        let b = self.base();
        let ix = s.ix as usize;
        if let Some(i) = self.copy_of(b, ix) {
            return &self.nodes[i];
        }
        b.nodes.get(ix).unwrap_or(&self.scratch)
    }

    #[inline]
    pub(super) fn node_opt(&self, s: Slot) -> Option<&Node> {
        let b = self.base();
        let ix = s.ix as usize;
        if let Some(i) = self.copy_of(b, ix) {
            return Some(&self.nodes[i]);
        }
        b.nodes.get(ix)
    }

    pub(super) fn fail(&mut self, why: &'static str) {
        if self.abort.is_none() {
            self.abort = Some(why);
        }
    }
}

fn scratch_node() -> Node {
    Node {
        id: super::NodeId(0),
        args: ValueId::UNIT,
        output: ValueId::UNIT,
        label: 0,
        sig: 0,
        op: super::OpId(0),
        unit: u32::MAX,
        parent: super::NIL,
        height: 0,
        floor: 0,
        epoch: 0,
        gen_: u32::MAX,
        aux: NO_AUX,
        on: super::NIL,
        state: State::Free,
        effect: 0,
        bound: 0,
        flags: 0,
        uses: Box::new([]),
        defs: Box::new([]),
    }
}

fn scratch_unit() -> Unit {
    Unit {
        root: TNIL,
        env: FxHashMap::default(),
        owner: super::NIL,
        accs: FxHashMap::default(),
        watches: BTreeMap::new(),
        max_span: 0,
        scans: std::collections::BTreeSet::new(),
        scan_span: 0,
        live: false,
        shifts: Vec::new(),
    }
}

impl Aux {
    /// A copy for an overlay to write (the kept run state stays with the
    /// original unless the overlay sets it).
    fn cow_clone(&self) -> Aux {
        Aux {
            children: self.children.clone(),
            waiters: self.waiters.clone(),
            out_readers: self.out_readers.clone(),
            publishes: self.publishes.clone(),
            contribs: self.contribs.clone(),
            folds: self.folds.clone(),
            static_contribs: self.static_contribs,
            decl_mask: self.decl_mask,
            claimed: self.claimed.clone(),
            pend_fields: self.pend_fields.clone(),
            claimed_by: self.claimed_by,
            kind: self.kind,
            body: self.body,
            assumers: self.assumers.clone(),
            assume_left: self.assume_left,
            read_predicted: self.read_predicted,
            watch: self.watch,
            watch_to: self.watch_to,
            scan: self.scan,
            reach1: self.reach1,
            stream: self.stream,
            diagnostics: self.diagnostics.clone(),
            kind_pending: self.kind_pending,
            decl_refuted: self.decl_refuted,
            narrowed: self.narrowed.clone(),
            suspended: Mutex::new(None),
        }
    }
}

impl RunResult {
    /// A copy without the kept run state (which cannot be copied): what a
    /// slice that aborts applies instead (the node then restarts rather
    /// than resumes).
    fn lite_clone(&self) -> RunResult {
        RunResult {
            node: self.node,
            slot: self.slot,
            epoch: self.epoch,
            speculative: self.speculative,
            read_unsettled: self.read_unsettled,
            read_predicted: self.read_predicted,
            end: self.end.clone(),
            raise: self.raise,
            raise_from: self.raise_from,
            uses: self.uses.clone(),
            defs: self.defs.clone(),
            children: self.children.clone(),
            follows: self.follows,
            child_heights: self.child_heights.clone(),
            child_ids: self.child_ids.clone(),
            old_sites: self.old_sites.clone(),
            env: self.env.clone(),
            env_found: self.env_found.clone(),
            publishes: self.publishes.clone(),
            contribs: self.contribs.clone(),
            contrib_folds: self.contrib_folds.clone(),
            watch: self.watch,
            scan: self.scan,
            narrow: self.narrow.clone(),
            effect: self.effect,
            kind: self.kind,
            claim: self.claim,
            claim_seen: self.claim_seen.clone(),
            stream: self.stream,
            touch_seq: self.touch_seq,
            relabels: self.relabels,
            binding_epoch: self.binding_epoch,
            assumptions: self.assumptions.clone(),
            sig: self.sig,
            error: self.error.clone(),
            diagnostics: self.diagnostics.clone(),
            suspended: None,
            resumed: self.resumed,
            restarted: self.restarted,
            memo: self.memo,
        }
    }
}

impl Report {
    /// Adds what an overlay counted.
    #[allow(clippy::too_many_lines)]
    fn absorb(&mut self, d: Report) {
        let Report {
            nodes_run,
            cutoffs,
            rebinds,
            created,
            freed,
            rounds,
            restarts,
            version: _,
            edits,
            edits_applied,
            settled,
            resettled,
            runs_by_op,
            runs,
            stops,
            raises,
            pending_waits,
            waits_by_kind,
            top_waited,
            ready_total,
            applied_widths,
            env_stalled,
            env_waits,
            env_provided,
            gated,
            speculations,
            confirmations,
            mispredictions,
            resumed,
            restarted,
            restarted_by,
            refuted_by_field,
            refuted_nodes,
            refutations_max,
            declarations_refuted,
            rerun_causes,
            static_contribs_refuted,
            predictions_confirmed,
            predictions_refuted,
            fuel_outs,
            steps,
            values_freed,
            scope_region,
            pending_sets,
            pending_sets_defining,
            text_memo_hits,
            text_memo_misses,
            app_memo_hits,
            app_memo_misses,
            spec_shape_changes,
            spec_stops,
            waits_by_op,
            critical_path: _,
            max_ready,
        } = d;
        self.nodes_run += nodes_run;
        self.cutoffs += cutoffs;
        self.rebinds += rebinds;
        self.created += created;
        self.freed += freed;
        self.rounds += rounds;
        self.restarts += restarts;
        self.edits += edits;
        self.edits_applied.extend(edits_applied);
        self.settled += settled;
        self.resettled += resettled;
        add_named(&mut self.runs_by_op, runs_by_op);
        self.runs += runs;
        self.stops += stops;
        self.raises += raises;
        self.pending_waits += pending_waits;
        add_named(&mut self.waits_by_kind, waits_by_kind);
        self.top_waited.extend(top_waited);
        self.ready_total += ready_total;
        for (i, w) in applied_widths.into_iter().enumerate() {
            if self.applied_widths.len() <= i {
                self.applied_widths.resize(i + 1, 0);
            }
            self.applied_widths[i] += w;
        }
        self.env_stalled += env_stalled;
        self.env_waits.extend(env_waits);
        self.env_provided += env_provided;
        self.gated += gated;
        self.speculations += speculations;
        self.confirmations += confirmations;
        self.mispredictions += mispredictions;
        self.resumed += resumed;
        self.restarted += restarted;
        add_named(&mut self.restarted_by, restarted_by);
        for (f, c) in refuted_by_field {
            match self.refuted_by_field.iter_mut().find(|e| e.0 == f) {
                Some(e) => e.1 += c,
                None => self.refuted_by_field.push((f, c)),
            }
        }
        self.refuted_nodes += refuted_nodes;
        self.refutations_max = self.refutations_max.max(refutations_max);
        self.declarations_refuted += declarations_refuted;
        self.rerun_causes.extend(rerun_causes);
        self.static_contribs_refuted += static_contribs_refuted;
        self.predictions_confirmed += predictions_confirmed;
        self.predictions_refuted += predictions_refuted;
        self.fuel_outs += fuel_outs;
        self.steps += steps;
        self.values_freed += values_freed;
        self.scope_region += scope_region;
        self.pending_sets += pending_sets;
        self.pending_sets_defining += pending_sets_defining;
        self.text_memo_hits += text_memo_hits;
        self.text_memo_misses += text_memo_misses;
        self.app_memo_hits += app_memo_hits;
        self.app_memo_misses += app_memo_misses;
        self.spec_shape_changes += spec_shape_changes;
        self.spec_stops += spec_stops;
        add_named(&mut self.waits_by_op, waits_by_op);
        self.max_ready = self.max_ready.max(max_ready);
    }
}

fn add_named(to: &mut Vec<(&'static str, u64)>, from: Vec<(&'static str, u64)>) {
    for (name, c) in from {
        match to.iter_mut().find(|e| e.0 == name) {
            Some(e) => e.1 += c,
            None => to.push((name, c)),
        }
    }
}

/// What applying a slice gave: its overlay (none if it aborted), else the
/// results to apply whole instead and the slots it held.
type SliceOut = (Option<Engine>, Vec<RunResult>, Vec<Slot>);

/// A slice to apply: its id, label bound, results and reserved slots.
struct Slice {
    sid: u32,
    bound: (u32, u64, u64),
    results: Vec<RunResult>,
    fresh: Vec<Slot>,
}

impl Engine {
    /// The engine an overlay reads through (itself, when not one).
    #[inline]
    pub(super) fn frozen(&self) -> &Engine {
        match &self.ov {
            Some(ov) => ov.base(),
            None => self,
        }
    }

    /// `nm` in an overlay: the slice's copy of an owned node (made on the
    /// first write).
    pub(super) fn nm_ov(&mut self, s: Slot) -> &mut Node {
        let owned = self.owned(s);
        let ov = self.ov.as_mut().expect("overlay");
        let base = ov.base;
        let b = base.get().expect("an overlay has a base");
        let ix = s.ix as usize;
        if let Some(i) = ov.copy_of(b, ix) {
            return &mut ov.nodes[i];
        }
        if !owned || ix >= b.nodes.len() || ov.nodes.len() >= LOCAL_MASK as usize {
            ov.fail("a write to a node outside the slice");
            ov.scratch = scratch_node();
            return &mut ov.scratch;
        }
        ov.nodes.push(b.nodes[ix].clone());
        ov.slots.push(s);
        let i = u32::try_from(ov.nodes.len() - 1).expect("copies");
        b.stage[ix].store((ov.sid << SID_SHIFT) | i, Ordering::Relaxed);
        ov.nodes.last_mut().expect("just pushed")
    }

    /// A reserved slot for a new node in an overlay, holding `node`.
    pub(super) fn fresh_ov(&mut self, node: Node) -> Slot {
        let ov = self.ov.as_mut().expect("overlay");
        let base = ov.base;
        let b = base.get().expect("an overlay has a base");
        let Some(s) = ov.fresh.pop() else {
            ov.fail("more new nodes than reserved");
            return Slot {
                ix: u32::MAX - 1,
                gen_: u32::MAX,
            };
        };
        if ov.nodes.len() >= LOCAL_MASK as usize {
            ov.fail("too many copies");
            return Slot {
                ix: u32::MAX - 1,
                gen_: u32::MAX,
            };
        }
        ov.nodes.push(Node { gen_: s.gen_, ..node });
        ov.slots.push(s);
        let i = u32::try_from(ov.nodes.len() - 1).expect("copies");
        b.stage[s.ix as usize].store((ov.sid << SID_SHIFT) | i, Ordering::Relaxed);
        s
    }

    pub(super) fn aux_of_ov(&self, s: Slot) -> Option<&Aux> {
        let ov = self.ov.as_ref().expect("overlay");
        let a = ov.node(s).aux;
        if a == NO_AUX {
            None
        } else if a & AUX_LOCAL != 0 {
            Some(&ov.aux[(a & !AUX_LOCAL) as usize])
        } else {
            ov.base().aux.get(a as usize)
        }
    }

    pub(super) fn ax_ov(&mut self, s: Slot) -> &mut Aux {
        let a = self.n(s).aux;
        if a != NO_AUX && a & AUX_LOCAL != 0 {
            let ov = self.ov.as_mut().expect("overlay");
            return &mut ov.aux[(a & !AUX_LOCAL) as usize];
        }
        // The node must be the slice's to hold a new aux id.
        let _ = self.nm(s);
        if self.ov.as_ref().expect("overlay").abort.is_some() {
            let ov = self.ov.as_mut().expect("overlay");
            ov.scratch_aux = Aux::default();
            return &mut ov.scratch_aux;
        }
        let ov = self.ov.as_mut().expect("overlay");
        let new = if a == NO_AUX {
            Aux::default()
        } else {
            ov.base().aux[a as usize].cow_clone()
        };
        ov.aux.push(new);
        ov.aux_home.push(a);
        ov.susp_set.push(false);
        let i = u32::try_from(ov.aux.len() - 1).expect("aux copies");
        self.nm(s).aux = i | AUX_LOCAL;
        let ov = self.ov.as_mut().expect("overlay");
        &mut ov.aux[i as usize]
    }

    /// Marks the aux of `s` as holding its own kept run state.
    pub(super) fn note_suspended(&mut self, s: Slot) {
        let a = self.n(s).aux;
        if let Some(ov) = &mut self.ov
            && a != NO_AUX
            && a & AUX_LOCAL != 0
        {
            ov.susp_set[(a & !AUX_LOCAL) as usize] = true;
        }
    }

    /// Ends an overlay's run with an abort for `why`.
    pub(super) fn ov_fail(&mut self, why: &'static str) {
        if let Some(ov) = &mut self.ov {
            ov.fail(why);
        }
    }

    pub(super) fn ov_aborted(&self) -> bool {
        self.ov.as_ref().is_some_and(|ov| ov.abort.is_some())
    }

    /// An overlay of `self` for slice `sid` over `bound`, creating nodes
    /// in `fresh`.
    #[allow(clippy::too_many_lines)]
    fn overlay(&self, sid: u32, bound: (u32, u64, u64), fresh: Vec<Slot>) -> Engine {
        Engine {
            values: self.values.clone(),
            ops: self.ops.clone(),
            nodes: Vec::new(),
            free_slots: Vec::new(),
            aux: Vec::new(),
            free_aux: Vec::new(),
            ids: OvMap::over(&self.ids),
            units: Vec::new(),
            free_units: Vec::new(),
            order: Arena::over(&self.order),
            accs_arena: Arena::over(&self.accs_arena),
            acc_cache: RwLock::new(FxHashMap::default()),
            monoids: self.monoids.clone(),
            defs_ix: LabelIx::over(&self.defs_ix),
            local_ix: LabelIx::over(&self.local_ix),
            stream_runs: OvMap::over(&self.stream_runs),
            touch_log: Vec::new(),
            in_eval: self.in_eval,
            relabels: self.relabels,
            binding_epoch: self.binding_epoch,
            fresh_scope: false,
            environment: FxHashMap::default(),
            env_next: None,
            env_absent: super::FxHashSet::default(),
            env_absent_next: None,
            env_waiters: FxHashMap::default(),
            env_provider: None,
            limits: self.limits.clone(),
            runs_in_eval: SlotCounts::over(&self.runs_in_eval),
            causes: OvMap::over(&self.causes),
            tracing: self.tracing,
            why: Rerun::Other,
            batch_defined: crate::hash::FxHashSet::default(),
            definer_due: std::sync::Mutex::default(),
            changed_from: ValueId::NONE,
            created_in_eval: OvMap::over(&self.created_in_eval),
            run_depths: Vec::new(),
            idle_rounds: 0,
            eval_runs: 0,
            #[cfg(not(target_arch = "wasm32"))]
            eval_start: None,
            global_ix: LabelIx::over(&self.global_ix),
            users: LabelIx::over(&self.users),
            chain_users: LabelIx::over(&self.chain_users),
            user_max: OvMap::over(&self.user_max),
            pending: LabelIx::over(&self.pending),
            pending_eff: LabelIx::over(&self.pending_eff),
            pending_f: LabelIx::over(&self.pending_f),
            pending_p: LabelIx::over(&self.pending_p),
            pending_p_lens: self.pending_p_lens.clone(),
            loop_vals: FxHashMap::default(),
            loop_init: FxHashMap::default(),
            loop_users: OvMap::over(&self.loop_users),
            loop_pubs: OvMap::over(&self.loop_pubs),
            memo: OvMap::over(&self.memo),
            text_memo: RwLock::new(FxHashMap::default()),
            statics: RwLock::new(FxHashMap::default()),
            predictions_seen: std::sync::atomic::AtomicBool::new(
                self.predictions_seen.load(std::sync::atomic::Ordering::Relaxed),
            ),
            queue: Vec::new(),
            queue_min: usize::MAX,
            queue_len: 0,
            order_dirty: Vec::new(),
            settle_pending: Vec::new(),
            parked: Vec::new(),
            parked_len: 0,
            roots: super::FxHashSet::default(),
            kind_field: self.kind_field,
            seqs: self.seqs.clone(),
            seq_start: self.seq_start,
            apps_live: 0,
            apps_at_collect: 0,
            claim_dirty: OvSet::over(&self.claim_dirty),
            errors: Vec::new(),
            node_errors: OvMap::over(&self.node_errors),
            level: self.level,
            report: Report::default(),
            touched: OvMap::over(&self.touched),
            max_rounds: self.max_rounds,
            client_serial: self.client_serial,
            feedback: self.feedback,
            loop_start: self.loop_start,
            spec_live: self.spec_live,
            spec_done: OvSet::over(&self.spec_done),
            slice: Some(bound),
            tail: Vec::new(),
            tail_out_changed: OvSet::over(&self.tail_out_changed),
            batch_lift: OvMap::over(&self.batch_lift),
            origins: SlotMap::over(&self.origins),
            wait_counts: FxHashMap::default(),
            edit_version: self.edit_version,
            version: self.version,
            absorbed_in_round: self.absorbed_in_round,
            marks: OvMap::over(&self.marks),
            marked_ops: self.marked_ops.clone(),
            settle_heap: std::collections::BinaryHeap::new(),
            settled: Vec::new(),
            settle_held: Vec::new(),
            done_heights: Vec::new(),
            done_max: 0,
            sweeps: self.sweeps,
            mispredicted: OvMap::over(&self.mispredicted),
            spec_blocked: FxHashMap::default(),
            pins: FxHashMap::default(),
            values_at_sweep: self.values_at_sweep,
            open_op: self.open_op,
            close_op: self.close_op,
            stage: Vec::new(),
            ov: Some(Box::new(Ov {
                base: Base::new(self),
                sid,
                nodes: Vec::new(),
                slots: Vec::new(),
                aux: Vec::new(),
                aux_home: Vec::new(),
                susp_set: Vec::new(),
                fresh,
                roots: FxHashMap::default(),
                acc_roots: FxHashMap::default(),
                order_log: Vec::new(),
                acc_log: Vec::new(),
                done_log: Vec::new(),
                abort: None,
                scratch: scratch_node(),
                scratch_aux: Aux::default(),
                scratch_unit: scratch_unit(),
                pending_p_lens0: self.pending_p_lens.clone(),
                spec_live0: self.spec_live,
                tracing0: self.tracing,
            })),
        }
    }

    /// Applies one slice's results through an overlay; `None` if it
    /// aborted (nothing of it then holds).
    fn run_slice(&self, sl: Slice) -> SliceOut {
        let mut e = self.overlay(sl.sid, sl.bound, sl.fresh);
        let mut fallback = Vec::with_capacity(sl.results.len());
        for r in sl.results {
            fallback.push(r.lite_clone());
            e.apply(r);
            if e.ov_aborted() {
                break;
            }
        }
        if !e.ov_aborted() {
            e.flush_order();
            e.compress_logs();
        }
        if e.binding_epoch != self.binding_epoch || e.relabels != self.relabels {
            e.ov_fail("the binding structure changed");
        }
        if e.ov_aborted() {
            let ov = e.ov.take().expect("overlay");
            for s in &ov.slots {
                self.stage[s.ix as usize].store(0, Ordering::Relaxed);
            }
            let mut slots = ov.fresh;
            slots.extend(ov.slots.iter().copied().filter(|s| self.n(*s).state == State::Free));
            return (None, fallback, slots);
        }
        (Some(e), Vec::new(), Vec::new())
    }

    /// Replaces an overlay's order and accumulator logs by the final
    /// state of what they touched: one run of new items and one batch of
    /// changed ones per unit, one value per accumulator key.
    fn compress_logs(&mut self) {
        let ov = self.ov.as_mut().expect("overlay");
        let log = std::mem::take(&mut ov.order_log);
        let mut new: Vec<(u32, u64)> = Vec::new();
        let mut changed: Vec<(u32, u64)> = Vec::new();
        for op in &log {
            match op {
                OrderOp::Insert(u, l, _) => new.push((*u, *l)),
                OrderOp::InsertRun(u, items) => new.extend(items.iter().map(|i| (*u, i.0))),
                OrderOp::Update(u, l, _) => changed.push((*u, *l)),
                OrderOp::SetMany(u, items) => changed.extend(items.iter().map(|i| (*u, i.0))),
                OrderOp::Remove(..) => {
                    // Not compressible (a removal then a new item at the
                    // label would read as a change): keep the log.
                    ov.order_log = log;
                    return;
                }
            }
        }
        new.sort_unstable();
        new.dedup();
        changed.sort_unstable();
        changed.dedup();
        changed.retain(|k| new.binary_search(k).is_err());
        let mut out = Vec::new();
        for (keys, run) in [(new, true), (changed, false)] {
            let mut i = 0;
            while i < keys.len() {
                let u = keys[i].0;
                let j = i + keys[i..].iter().take_while(|k| k.0 == u).count();
                let root = self.root(u);
                let items: Vec<(u64, OItem)> = keys[i..j]
                    .iter()
                    .filter_map(|k| self.order.get(root, k.1).map(|it| (k.1, *it)))
                    .collect();
                out.push(if run {
                    OrderOp::InsertRun(u, items)
                } else {
                    OrderOp::SetMany(u, items)
                });
                i = j;
            }
        }
        let ov = self.ov.as_mut().expect("overlay");
        ov.order_log = out;
        // Accumulators: the last value per key, in key order.
        let mut acc = std::mem::take(&mut ov.acc_log);
        acc.reverse();
        let mut seen = super::FxHashSet::default();
        acc.retain(|e| seen.insert((e.0, e.1, e.2)));
        acc.reverse();
        ov.acc_log = acc;
    }

    /// Writes the slices' overlays into the engine, each structure merged
    /// from every slice in slice order, the structures concurrently.
    #[allow(clippy::too_many_lines)]
    fn merge_all(&mut self, slices: Vec<Engine>, exec: &mut dyn Executor) {
        let mut parts: Vec<Parts> = slices.into_iter().map(Parts::of).collect();
        // Aux ids for the auxes the slices made: from the free list, in
        // slice order (sequential, cheap).
        let mut aux_ids: Vec<Vec<u32>> = Vec::with_capacity(parts.len());
        for p in &parts {
            let mut ids = Vec::with_capacity(p.ov.aux_home.len());
            for &home in &p.ov.aux_home {
                ids.push(if home == NO_AUX {
                    if let Some(i) = self.free_aux.pop() {
                        i
                    } else {
                        self.aux.push(Aux::default());
                        u32::try_from(self.aux.len() - 1).expect("aux overflow")
                    }
                } else {
                    home
                });
            }
            aux_ids.push(ids);
        }
        for p in &parts {
            self.free_slots.extend(p.ov.fresh.iter().map(|s| s.ix));
            self.touch_log.extend(p.touch_log.iter().copied());
            self.tracing = self.tracing.wrapping_add(p.tracing.wrapping_sub(p.ov.tracing0));
            self.spec_live = self.spec_live.wrapping_add(p.spec_live.wrapping_sub(p.ov.spec_live0));
            self.eval_runs += p.eval_runs;
            self.queue_len = self.queue_len.wrapping_add(p.queue_len);
            self.parked_len = self.parked_len.wrapping_add(p.parked_len);
        }
        let mut node_parts = Vec::with_capacity(parts.len());
        let mut order_parts = Vec::with_capacity(parts.len());
        let mut users = Vec::new();
        let mut chain = Vec::new();
        let mut defs = Vec::new();
        let mut pend = Vec::new();
        let mut ids = Vec::new();
        let mut touched = Vec::new();
        let mut slots = Vec::new();
        let mut maps = Vec::new();
        let mut logs = Vec::new();
        for (p, a) in parts.drain(..).zip(aux_ids) {
            let Parts {
                ov,
                ids: p_ids,
                defs_ix,
                local_ix,
                global_ix,
                users: p_users,
                chain_users,
                pending,
                pending_eff,
                pending_f,
                pending_p,
                pending_p_lens,
                touched: p_touched,
                origins,
                runs_in_eval,
                small,
                logs: p_logs,
                ..
            } = p;
            let Ov {
                nodes,
                slots: ov_slots,
                aux,
                aux_home,
                susp_set,
                order_log,
                acc_log,
                done_log,
                pending_p_lens0,
                ..
            } = *ov;
            node_parts.push((nodes, ov_slots, aux, aux_home, susp_set, a));
            order_parts.push((order_log, acc_log, done_log));
            users.push(p_users);
            chain.push(chain_users);
            defs.push((defs_ix, local_ix, global_ix));
            pend.push((
                pending,
                pending_eff,
                pending_f,
                pending_p,
                pending_p_lens,
                pending_p_lens0,
            ));
            ids.push(p_ids);
            touched.push(p_touched);
            slots.push((origins, runs_in_eval));
            let mut small = small;
            self.spec_blocked.extend(std::mem::take(&mut small.spec_blocked));
            maps.push(small);
            logs.push(p_logs);
        }
        let Engine {
            nodes: e_nodes,
            aux: e_aux,
            stage,
            order,
            accs_arena,
            units,
            monoids,
            done_heights,
            done_max,
            users: e_users,
            chain_users: e_chain,
            defs_ix: e_defs,
            local_ix: e_local,
            global_ix: e_global,
            pending: e_pending,
            pending_eff: e_pending_eff,
            pending_f: e_pending_f,
            pending_p: e_pending_p,
            pending_p_lens: e_plens,
            ids: e_ids,
            touched: e_touched,
            origins: e_origins,
            runs_in_eval: e_runs,
            stream_runs,
            causes,
            created_in_eval,
            loop_users,
            loop_pubs,
            memo,
            claim_dirty,
            node_errors,
            spec_done,
            tail_out_changed,
            batch_lift,
            marks,
            mispredicted,
            user_max,
            wait_counts,
            queue,
            queue_min,
            parked,
            settle_pending,
            settle_heap,
            errors,
            tail,
            report,
            ..
        } = self;
        let stage: &[std::sync::atomic::AtomicU32] = stage;
        // Node records and auxes: disjoint slots, written per slice in
        // parallel (old records dropped there too).
        let mut node_writes = Vec::with_capacity(node_parts.len());
        let mut aux_writes = Vec::with_capacity(node_parts.len());
        for (nodes, slots, aux, home, set, ids) in node_parts {
            let mut aw = Vec::with_capacity(aux.len());
            for (((mut a, home), set), &id) in aux.into_iter().zip(home).zip(set).zip(&ids) {
                if home != NO_AUX && !set {
                    a.suspended = std::mem::take(&mut e_aux[home as usize].suspended);
                }
                aw.push((id as usize, a));
            }
            let mut nw = Vec::with_capacity(nodes.len());
            for (mut n, s) in nodes.into_iter().zip(slots) {
                if n.aux != NO_AUX && n.aux & AUX_LOCAL != 0 {
                    n.aux = ids[(n.aux & !AUX_LOCAL) as usize];
                }
                stage[s.ix as usize].store(0, Ordering::Relaxed);
                nw.push((s.ix as usize, n));
            }
            node_writes.push(nw);
            aux_writes.push(aw);
        }
        {
            let mut logs: Vec<&mut Vec<OrderOp>> = order_parts.iter_mut().map(|p| &mut p.0).collect();
            replay_order_par(order, units, &mut logs, exec);
        }
        crate::par::scatter(e_nodes, node_writes, exec);
        crate::par::scatter(e_aux, aux_writes, exec);
        let mut tasks: Vec<Box<dyn FnOnce() + Send + '_>> = Vec::new();
        tasks.push(Box::new(move || {
            // One pass per unit over every slice's changed items (sorted),
            // after the new runs.
            let mut changed: Vec<(u32, u64, OItem)> = Vec::new();
            for (order_log, _, _) in &mut order_parts {
                for op in std::mem::take(order_log) {
                    match op {
                        OrderOp::SetMany(u, items) => changed.extend(items.into_iter().map(|(l, i)| (u, l, i))),
                        op => replay_order(order, units, op),
                    }
                }
            }
            changed.sort_unstable_by_key(|e| (e.0, e.1));
            let mut i = 0;
            while i < changed.len() {
                let u = changed[i].0;
                let j = i + changed[i..].iter().take_while(|e| e.0 == u).count();
                let items: Vec<(u64, OItem)> = changed[i..j].iter().map(|e| (e.1, e.2)).collect();
                replay_order(order, units, OrderOp::SetMany(u, items));
                i = j;
            }
            for (_, acc_log, done_log) in order_parts {
                for (u, f, l, v) in acc_log {
                    if monoids.contains_key(&f) {
                        replay_acc(accs_arena, units, u, f, l, v);
                    }
                }
                for (h, up) in done_log {
                    done_count_in(done_heights, done_max, h, up);
                }
            }
        }));
        tasks.push(Box::new(move || {
            for x in users {
                x.merge_into(e_users);
            }
        }));
        tasks.push(Box::new(move || {
            for x in chain {
                x.merge_into(e_chain);
            }
        }));
        tasks.push(Box::new(move || {
            for (d, l, g) in defs {
                d.merge_into(e_defs);
                l.merge_into(e_local);
                g.merge_into(e_global);
            }
        }));
        tasks.push(Box::new(move || {
            for (p, pe, pf, pp, lens, lens0) in pend {
                p.merge_into(e_pending);
                pe.merge_into(e_pending_eff);
                pf.merge_into(e_pending_f);
                pp.merge_into(e_pending_p);
                merge_counts(e_plens, &lens, &lens0);
            }
        }));
        tasks.push(Box::new(move || {
            for x in ids {
                x.merge_into(e_ids);
            }
        }));
        tasks.push(Box::new(move || {
            for x in touched {
                x.merge_into(e_touched);
            }
        }));
        tasks.push(Box::new(move || {
            for (o, r) in slots {
                o.merge_into(e_origins);
                r.merge_into(e_runs);
            }
        }));
        tasks.push(Box::new(move || {
            for m in maps {
                m.stream_runs.merge_into(stream_runs);
                m.causes.merge_into(causes);
                m.created_in_eval.merge_into(created_in_eval);
                m.loop_users.merge_into(loop_users);
                m.loop_pubs.merge_into(loop_pubs);
                m.memo.merge_into(memo);
                m.claim_dirty.merge_into(claim_dirty);
                m.node_errors.merge_into(node_errors);
                m.spec_done.merge_into(spec_done);
                m.tail_out_changed.merge_into(tail_out_changed);
                m.batch_lift.merge_into(batch_lift);
                m.marks.merge_into(marks);
                m.mispredicted.merge_into(mispredicted);
                for (k, v) in m.user_max.local() {
                    let x = user_max.entry_or(*k, || 0);
                    *x = (*x).max(*v);
                }
                for (s, c) in m.wait_counts {
                    *wait_counts.entry(s).or_insert(0) += c;
                }
            }
        }));
        tasks.push(Box::new(move || {
            for l in logs {
                for (h, b) in l.queue.into_iter().enumerate() {
                    if b.is_empty() {
                        continue;
                    }
                    if queue.len() <= h {
                        queue.resize_with(h + 1, Vec::new);
                    }
                    queue[h].extend(b);
                    *queue_min = (*queue_min).min(h);
                }
                parked.extend(l.parked);
                settle_pending.extend(l.settle_pending);
                settle_heap.extend(l.settle_heap);
                errors.extend(l.errors);
                tail.extend(l.tail);
                report.absorb(l.report);
            }
        }));
        let tasks: Vec<Mutex<Option<Task<'_>>>> = tasks.into_iter().map(|t| Mutex::new(Some(t))).collect();
        exec.for_each(tasks.len(), &|i| {
            if let Some(t) = tasks[i].lock().unwrap_or_else(PoisonError::into_inner).take() {
                t();
            }
        });
    }

    /// Applies a batch's results in slices (see the module comment):
    /// `slices` are the consecutive runs of `results` to apply each
    /// through its overlay, by label bound; the rest apply whole after.
    pub(super) fn apply_sliced(
        &mut self,
        slices: Vec<((u32, u64, u64), Vec<RunResult>)>,
        mut wholes: Vec<RunResult>,
        exec: &mut dyn Executor,
    ) {
        // Reserve the slots the slices' new nodes may take.
        let mut prepared = Vec::with_capacity(slices.len());
        for (i, (bound, results)) in slices.into_iter().enumerate() {
            let need: usize = results.iter().map(|r| r.children.len()).sum();
            let mut fresh = Vec::with_capacity(need);
            for _ in 0..need {
                let s = if let Some(ix) = self.free_slots.pop() {
                    Slot {
                        ix,
                        gen_: self.nodes[ix as usize].gen_,
                    }
                } else {
                    let mut n = super::slice::scratch_node();
                    n.gen_ = 0;
                    self.nodes.push(n);
                    Slot {
                        ix: u32::try_from(self.nodes.len() - 1).expect("too many nodes"),
                        gen_: 0,
                    }
                };
                fresh.push(s);
            }
            // Taken from the end: in reservation order.
            fresh.reverse();
            prepared.push(Slice {
                sid: u32::try_from(i + 1).expect("slices"),
                bound,
                results,
                fresh,
            });
        }
        if self.stage.len() < self.nodes.len() {
            self.stage.resize_with(self.nodes.len(), Default::default);
        }
        let outs: Vec<SliceOut> = {
            span!(Slices);
            let ins: Vec<Mutex<Option<Slice>>> = prepared.into_iter().map(|sl| Mutex::new(Some(sl))).collect();
            let outs: Vec<Mutex<Option<SliceOut>>> = ins.iter().map(|_| Mutex::new(None)).collect();
            let this: &Engine = self;
            exec.for_each(ins.len(), &|i| {
                let sl = ins[i]
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                    .expect("a slice");
                let out = this.run_slice(sl);
                *outs[i].lock().unwrap_or_else(PoisonError::into_inner) = Some(out);
            });
            outs.into_iter()
                .map(|o| {
                    o.into_inner()
                        .unwrap_or_else(PoisonError::into_inner)
                        .expect("every slice ran")
                })
                .collect()
        };
        #[cfg(feature = "profile")]
        let merge_span = crate::obs::enter_impl(crate::obs::Span::Merge);
        let mut aborted = Vec::new();
        let mut merged = Vec::with_capacity(outs.len());
        for (e, fallback, slots) in outs {
            if let Some(e) = e {
                merged.push(e);
            } else {
                count!(SliceAbort);
                self.free_slots.extend(slots.iter().map(|s| s.ix));
                aborted.extend(fallback);
            }
        }
        self.merge_all(merged, exec);
        #[cfg(feature = "profile")]
        drop(merge_span);
        self.drain_tail();
        // Whole results and those of aborted slices, in text order.
        if !aborted.is_empty() {
            wholes.extend(aborted);
            let this: &Engine = self;
            wholes.sort_by_cached_key(|r| match this.nodes.get(r.slot.ix as usize) {
                Some(n) if this.live(r.slot) => (n.unit, n.label, r.node),
                _ => (u32::MAX, 0, r.node),
            });
        }
        for r in wholes {
            self.apply(r);
        }
    }
}

/// The maps an overlay merges by plain replacement (and two by sum and
/// maximum).
struct SmallMaps {
    stream_runs: OvMap<Slot, u32>,
    causes: OvMap<Slot, Vec<super::Cause>>,
    created_in_eval: OvMap<Slot, (u32, Vec<(super::OpId, ValueId)>)>,
    loop_users: OvMap<Field, super::FxHashSet<Slot>>,
    loop_pubs: OvMap<Field, BTreeMap<super::NodeId, ValueId>>,
    memo: OvMap<(super::OpId, ValueId), Slot>,
    claim_dirty: OvSet<Slot>,
    node_errors: OvMap<Slot, super::EvalError>,
    spec_done: OvSet<Slot>,
    tail_out_changed: OvSet<Slot>,
    batch_lift: OvMap<Slot, super::Lifted>,
    marks: OvMap<Slot, Option<(u64, ValueId)>>,
    mispredicted: OvMap<Slot, u32>,
    spec_blocked: FxHashMap<Slot, (Slot, super::NodeId)>,
    user_max: OvMap<(u32, Field), u64>,
    wait_counts: FxHashMap<Slot, u64>,
}

/// What an overlay appended.
struct Logs {
    queue: Vec<Vec<(super::NodeId, Slot)>>,
    parked: Vec<Slot>,
    settle_pending: Vec<(u32, u64)>,
    settle_heap: std::collections::BinaryHeap<std::cmp::Reverse<(u32, Slot, u32)>>,
    errors: Vec<super::EvalError>,
    tail: Vec<super::Tail>,
    report: Report,
}

/// An overlay taken apart for merging.
struct Parts {
    ov: Box<Ov>,
    ids: OvMap<super::NodeId, Slot>,
    defs_ix: LabelIx<(u32, Field)>,
    local_ix: LabelIx<(u32, Field, i32)>,
    global_ix: LabelIx<(u32, Field)>,
    users: LabelIx<(u64, Field)>,
    chain_users: LabelIx<(u64, Field)>,
    pending: LabelIx<u32>,
    pending_eff: LabelIx<u32>,
    pending_f: LabelIx<(u32, Field)>,
    pending_p: LabelIx<(u32, u64)>,
    pending_p_lens: BTreeMap<usize, u32>,
    touched: OvMap<super::NodeId, super::Snap>,
    origins: SlotMap<super::Origin>,
    runs_in_eval: SlotCounts,
    small: SmallMaps,
    logs: Logs,
    touch_log: Vec<(u32, u64)>,
    tracing: u32,
    spec_live: u32,
    eval_runs: u64,
    queue_len: usize,
    parked_len: usize,
}

impl Parts {
    fn of(e: Engine) -> Parts {
        let Engine {
            ids,
            defs_ix,
            local_ix,
            stream_runs,
            touch_log,
            runs_in_eval,
            causes,
            tracing,
            created_in_eval,
            eval_runs,
            global_ix,
            users,
            chain_users,
            user_max,
            pending,
            pending_eff,
            pending_f,
            pending_p,
            pending_p_lens,
            loop_users,
            loop_pubs,
            memo,
            queue,
            queue_len,
            order_dirty,
            settle_pending,
            parked,
            parked_len,
            claim_dirty,
            errors,
            node_errors,
            report,
            touched,
            spec_live,
            spec_done,
            tail,
            tail_out_changed,
            batch_lift,
            origins,
            wait_counts,
            marks,
            settle_heap,
            mispredicted,
            spec_blocked,
            ov,
            ..
        } = e;
        debug_assert!(order_dirty.is_empty(), "an overlay's order is flushed");
        Parts {
            ov: ov.expect("overlay"),
            ids,
            defs_ix,
            local_ix,
            global_ix,
            users,
            chain_users,
            pending,
            pending_eff,
            pending_f,
            pending_p,
            pending_p_lens,
            touched,
            origins,
            runs_in_eval,
            small: SmallMaps {
                stream_runs,
                causes,
                created_in_eval,
                loop_users,
                loop_pubs,
                memo,
                claim_dirty,
                node_errors,
                spec_done,
                tail_out_changed,
                batch_lift,
                marks,
                mispredicted,
                spec_blocked,
                user_max,
                wait_counts,
            },
            logs: Logs {
                queue,
                parked,
                settle_pending,
                settle_heap,
                errors,
                tail,
                report,
            },
            touch_log,
            tracing,
            spec_live,
            eval_runs,
            queue_len,
            parked_len,
        }
    }
}

/// `Engine::ord_op` on the engine's order and units, for a merge.
#[allow(clippy::needless_pass_by_value)]
fn replay_order(order: &mut OArena, units: &mut [Unit], op: OrderOp) {
    let unit = op.unit() as usize;
    let root = units[unit].root;
    let r = match &op {
        OrderOp::Insert(_, l, item) => order.insert(&super::OAug, root, *l, *item),
        OrderOp::InsertRun(_, items) => order.insert_run(&super::OAug, root, items),
        OrderOp::Remove(_, l) => order.remove(&super::OAug, root, *l).0,
        OrderOp::Update(_, l, item) => {
            let item = *item;
            order.update(&super::OAug, root, *l, &mut |i| *i = item);
            root
        }
        OrderOp::SetMany(_, items) => {
            order.set_many(&super::OAug, root, items);
            root
        }
    };
    units[unit].root = r;
}

/// `Engine::acc_set` on the engine's accumulators and units, for a merge.
fn replay_acc(
    accs: &mut Arena<ValueId, super::AccSum>,
    units: &mut [Unit],
    unit: u32,
    f: Field,
    label: u64,
    v: Option<ValueId>,
) {
    let u = &mut units[unit as usize];
    let root = u.accs.get(&f).copied().unwrap_or(TNIL);
    let r = match v {
        Some(v) if accs.update(&super::AccAug, root, label, &mut |x| *x = v) => root,
        Some(v) => accs.insert(&super::AccAug, root, label, v),
        None => accs.remove(&super::AccAug, root, label).0,
    };
    u.accs.insert(f, r);
}

/// `Engine::done_count` on the counts themselves.
fn done_count_in(heights: &mut Vec<u32>, max: &mut usize, h: u32, up: bool) {
    let h = h as usize;
    if heights.len() <= h {
        heights.resize(h + 1, 0);
    }
    if up {
        heights[h] += 1;
        *max = (*max).max(h);
    } else {
        heights[h] = heights[h].saturating_sub(1);
    }
}

/// Adds to `base` what a copy of it gained or lost (`now` against `was`).
fn merge_counts(base: &mut BTreeMap<usize, u32>, now: &BTreeMap<usize, u32>, was: &BTreeMap<usize, u32>) {
    if now == was {
        return;
    }
    let mut keys: Vec<usize> = now.keys().chain(was.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();
    for k in keys {
        let (n, w) = (
            i64::from(now.get(&k).copied().unwrap_or(0)),
            i64::from(was.get(&k).copied().unwrap_or(0)),
        );
        if n == w {
            continue;
        }
        let c = base.entry(k).or_insert(0);
        *c = u32::try_from(i64::from(*c) + n - w).unwrap_or(0);
        if *c == 0 {
            base.remove(&k);
        }
    }
}

/// The labels an order op touches, lowest and highest.
fn op_span(op: &OrderOp) -> Option<(u64, u64)> {
    match op {
        OrderOp::Insert(_, l, _) | OrderOp::Remove(_, l) | OrderOp::Update(_, l, _) => Some((*l, *l)),
        OrderOp::InsertRun(_, items) | OrderOp::SetMany(_, items) => {
            let lo = items.iter().map(|i| i.0).min()?;
            let hi = items.iter().map(|i| i.0).max()?;
            Some((lo, hi))
        }
    }
}

fn op_item(op: &OrderOp) -> Option<OItem> {
    match op {
        OrderOp::Insert(_, _, i) | OrderOp::Update(_, _, i) => Some(*i),
        OrderOp::InsertRun(_, items) | OrderOp::SetMany(_, items) => items.first().map(|i| i.1),
        OrderOp::Remove(..) => None,
    }
}

fn op_size(op: &OrderOp) -> usize {
    match op {
        OrderOp::InsertRun(_, items) | OrderOp::SetMany(_, items) => items.len(),
        _ => 1,
    }
}

/// A slice's ops in one unit: (slice, lowest label, highest label, size).
type Seg = (usize, u64, u64, usize);
type OArena = Arena<OItem, super::OSum, super::Br>;
type PieceCell = std::sync::Mutex<Option<(OArena, u32, Vec<OrderOp>)>>;
type Task<'a> = Box<dyn FnOnce() + Send + 'a>;

/// Replays the slices' order logs in parallel where they can be: per
/// unit, when the slices' ops lie in increasing, disjoint label ranges,
/// the unit's tree is split at the ranges' starts, each slice's ops are
/// replayed into its own piece concurrently, and the pieces are joined.
/// The logs replayed are emptied; the rest stay for the sequential replay.
#[allow(clippy::too_many_lines)]
fn replay_order_par(order: &mut OArena, units: &mut [Unit], logs: &mut [&mut Vec<OrderOp>], exec: &mut dyn Executor) {
    let min_ops: usize = std::env::var("DEFUSE_PAR_ORDER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4096);
    // Per unit: (slice, lo, hi, size) for each slice whose ops all lie in
    // that unit.
    let mut segs: BTreeMap<u32, Vec<Seg>> = BTreeMap::new();
    let mut fill = None;
    for (s, log) in logs.iter().enumerate() {
        let Some(u) = log.first().map(OrderOp::unit) else {
            continue;
        };
        let mut span: Option<(u64, u64)> = None;
        let mut size = 0;
        let mut ok = true;
        for op in log.iter() {
            if op.unit() != u {
                ok = false;
                break;
            }
            if let Some((lo, hi)) = op_span(op) {
                span = Some(span.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))));
            }
            if fill.is_none() {
                fill = op_item(op);
            }
            size += op_size(op);
        }
        if let (true, Some((lo, hi))) = (ok, span) {
            segs.entry(u).or_default().push((s, lo, hi, size));
        }
    }
    let Some(fill) = fill else { return };
    // Keep the units whose segments are worth it and in order.
    let mut plan: Vec<(u32, Vec<Seg>)> = Vec::new();
    for (u, v) in segs {
        let total: usize = v.iter().map(|e| e.3).sum();
        let ordered = v.windows(2).all(|w| w[0].0 < w[1].0 && w[0].2 < w[1].1);
        if v.len() >= 2 && total >= min_ops && ordered {
            plan.push((u, v));
        }
    }
    if plan.is_empty() {
        return;
    }
    // Split each unit's tree at its segments' starts: piece k holds the
    // keys of [lo_k, lo_{k+1}) (the first everything below, the last
    // everything above).
    let mut roots: Vec<u32> = Vec::new();
    let mut reserve: Vec<(usize, usize)> = Vec::new();
    let mut work: Vec<Vec<OrderOp>> = Vec::new();
    for (u, v) in &plan {
        let mut rest = units[*u as usize].root;
        let mut cut = Vec::with_capacity(v.len());
        for (k, e) in v.iter().enumerate().rev() {
            if k == 0 {
                cut.push(rest);
            } else {
                let (l, r) = order.split(&super::OAug, rest, e.1);
                cut.push(r);
                rest = l;
            }
        }
        cut.reverse();
        for (e, root) in v.iter().zip(cut) {
            roots.push(root);
            reserve.push((e.3 / 8 + 32, e.3 / 32 + 32));
            work.push(std::mem::take(logs[e.0]));
        }
    }
    let mut pieces = order.pieces(&super::OAug, fill, &reserve);
    let parts = std::mem::take(&mut pieces.parts);
    let cells: Vec<PieceCell> = parts
        .into_iter()
        .zip(roots)
        .zip(work)
        .map(|((a, r), w)| std::sync::Mutex::new(Some((a, r, w))))
        .collect();
    exec.for_each(cells.len(), &|i| {
        let mut cell = cells[i].lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (a, root, ops) = cell.as_mut().expect("piece");
        for op in std::mem::take(ops) {
            *root = match &op {
                OrderOp::Insert(_, l, item) => a.insert(&super::OAug, *root, *l, *item),
                OrderOp::InsertRun(_, items) => a.insert_run(&super::OAug, *root, items),
                OrderOp::Remove(_, l) => a.remove(&super::OAug, *root, *l).0,
                OrderOp::Update(_, l, item) => {
                    let item = *item;
                    a.update(&super::OAug, *root, *l, &mut |i| *i = item);
                    *root
                }
                OrderOp::SetMany(_, items) => {
                    a.set_many(&super::OAug, *root, items);
                    *root
                }
            };
        }
    });
    let mut done = Vec::with_capacity(cells.len());
    let mut roots = Vec::with_capacity(cells.len());
    for c in cells {
        let (a, r, _) = c
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .expect("piece");
        done.push(a);
        roots.push(r);
    }
    pieces.finish(done);
    let mut roots = roots.into_iter();
    for (u, v) in &plan {
        let mut root = TNIL;
        for _ in v {
            let r = roots.next().expect("piece root");
            root = order.join(&super::OAug, root, r);
        }
        units[*u as usize].root = root;
    }
}
