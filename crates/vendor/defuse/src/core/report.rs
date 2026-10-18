//! The report of an evaluation: the canonical counters (from the state
//! before and after), the schedule's counters, and the critical chain
//! (from the final committed reads).

use std::sync::atomic::Ordering;

use super::edit::Engine;
use super::{DAns, F_DONE, NIL, Pred, S_FREE, S_INERT, S_MARKER, Slot, lock};
use crate::api::{Cause, NodeId, Report};
use crate::values::FxHashMap;

fn sorted(m: &FxHashMap<&'static str, u64>) -> Vec<(&'static str, u64)> {
    let mut v: Vec<(&'static str, u64)> = m.iter().map(|(k, v)| (*k, *v)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v
}

/// The kept heights: each node's height, the cause and the node it
/// comes from; the nodes by height; who reads each node.
#[derive(Default)]
pub(crate) struct Heights {
    init: bool,
    /// By slot.
    of: Vec<Option<(u32, Cause, Slot)>>,
    /// How many nodes have each height.
    lev: Vec<u32>,
    /// By slot: the nodes whose heights read it.
    rev: Vec<Vec<Slot>>,
}

impl Heights {
    /// The greatest height held.
    #[allow(clippy::cast_possible_truncation)]
    fn top(&self) -> u32 {
        self.lev.iter().rposition(|&c| c > 0).unwrap_or(0) as u32
    }
}

/// A node on the height walk's stack: its reads and how many are done.
type Frame = (Slot, Vec<(Slot, Cause)>, usize);

/// A node and its reads (gathered ahead of the height walk).
type NodeDeps = (Slot, Vec<(Slot, Cause)>);

fn rank(c: &Cause) -> u8 {
    match c {
        Cause::Changed { .. } => 0,
        Cause::Output { .. } => 1,
        Cause::Range { .. } => 2,
        Cause::Woken { .. } => 3,
        _ => 4,
    }
}

impl Engine {
    pub(crate) fn fill_report(&mut self, rep: &mut Report, exec: Option<&mut dyn crate::api::Executor>) {
        let sh = &self.sh;
        let c = &sh.c;
        // Executions: complete runs (a stopped attempt is not one).
        rep.runs = c.commits.load(Ordering::Relaxed);
        rep.stops = c.stops.load(Ordering::Relaxed);
        rep.raises = c.raises.load(Ordering::Relaxed);
        rep.pending_waits = c.pending_waits.load(Ordering::Relaxed);
        rep.pending_sets_defining = c.pending_defining.load(Ordering::Relaxed);
        rep.speculations = c.speculations.load(Ordering::Relaxed);
        rep.confirmations = c.confirmations.load(Ordering::Relaxed);
        rep.mispredictions = c.mispredictions.load(Ordering::Relaxed);
        rep.fuel_outs = c.fuel_outs.load(Ordering::Relaxed);
        rep.spec_stops = c.spec_stops.load(Ordering::Relaxed);
        rep.claims_refuted = c.claims_refuted.load(Ordering::Relaxed);
        rep.defs_refuted = c.defs_refuted.load(Ordering::Relaxed);
        rep.effects_refuted = c.effects_refuted.load(Ordering::Relaxed);
        rep.claim_siblings = c.claim_siblings.load(Ordering::Relaxed);
        rep.sole_successor = c.sole_successor.load(Ordering::Relaxed);
        rep.many_successors = c.many_successors.load(Ordering::Relaxed);
        rep.kept_successors = c.kept_successors.load(Ordering::Relaxed);
        rep.resumed = c.resumed.load(Ordering::Relaxed);
        rep.restarted = c.restarted.load(Ordering::Relaxed);
        rep.declarations_refuted = c.declarations_refuted.load(Ordering::Relaxed);
        rep.static_contribs_refuted = c.static_contribs_refuted.load(Ordering::Relaxed);
        rep.env_provided = c.env_provided.load(Ordering::Relaxed);
        rep.scope_region = c.scope_region.load(Ordering::Relaxed);
        rep.gated = c.gated.load(Ordering::Relaxed);
        rep.predictions_confirmed = c.predictions_confirmed.load(Ordering::Relaxed);
        rep.predictions_refuted = c.predictions_refuted.load(Ordering::Relaxed);
        rep.nodes_conservative = c.nodes_conservative.load(Ordering::Relaxed);
        rep.realised_critical_path = u32::try_from(c.realised.load(Ordering::Relaxed)).unwrap_or(u32::MAX);
        rep.realised_critical_time = std::time::Duration::from_nanos(c.realised_ns.load(Ordering::Relaxed));
        {
            // The last reason per node that ran in this evaluation.
            let reruns = std::mem::take(&mut *lock(&sh.reruns));
            let ep = sh.epoch.load(Ordering::Relaxed);
            // Each run takes the reason last recorded before it.
            let mut pending: FxHashMap<Slot, crate::api::Rerun> = FxHashMap::default();
            let mut last: FxHashMap<Slot, (usize, crate::api::Rerun)> = FxHashMap::default();
            for (i, (s, why)) in reruns.into_iter().enumerate() {
                match why {
                    Some(w) => {
                        pending.insert(s, w);
                    }
                    None => {
                        if let Some(w) = pending.remove(&s) {
                            last.insert(s, (i, w));
                        }
                    }
                }
            }
            let mut out: Vec<(usize, NodeId, crate::api::Rerun)> = Vec::new();
            for (s, (i, mut why)) in last {
                let r = sh.rec(s);
                if r.st() == S_FREE {
                    continue;
                }
                let d = r.data();
                if d.run_epoch != ep {
                    continue;
                }
                if let crate::api::Rerun::Changed { field, new, .. } = &mut why {
                    *new = super::Shared::read_value(&d, *field).unwrap_or(crate::values::ValueId::UNIT);
                }
                out.push((i, sh.node_id(s), why));
            }
            out.sort_by_key(|e| e.0);
            rep.rerun_causes = out.into_iter().map(|e| (e.1, e.2)).collect();
        }
        rep.created = c.created.load(Ordering::Relaxed);
        rep.freed = c.freed.load(Ordering::Relaxed);
        {
            let t = sh.tallies();
            rep.runs_by_op = sorted(&t.runs_by_op);
            let mut times: Vec<(NodeId, std::time::Duration)> = t
                .run_ns
                .iter()
                .map(|(&s, &ns)| (sh.node_id(s), std::time::Duration::from_nanos(ns)))
                .collect();
            times.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            rep.run_times = times;
            rep.waits_by_op = sorted(&t.waits_by_op);
            rep.waits_by_kind = sorted(&t.waits_by_kind);
            rep.restarted_by = sorted(&t.restarted_by);
            let mut top: Vec<(NodeId, u64)> = t.waited.iter().map(|(s, n)| (sh.node_id(*s), *n)).collect();
            top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.0.cmp(&b.0.0)));
            top.truncate(16);
            rep.top_waited = top;
            let mut rf: Vec<(crate::values::Field, u64)> = t.refuted_by_field.iter().map(|(f, n)| (*f, *n)).collect();
            rf.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.0.raw().cmp(&b.0.0.raw())));
            rep.refuted_by_field = rf;
            rep.refuted_nodes = t.refuted_nodes.len() as u64;
            rep.refutations_max = t.refuted_nodes.values().copied().max().unwrap_or(0);
        }
        // Canonical counters: the snapshots against the state now.
        let mut touched = std::mem::take(&mut *lock(&sh.touched));
        touched.sort_unstable();
        touched.dedup();
        let mut snaps = std::mem::take(&mut *lock(&sh.snaps));
        let mut ran: Vec<Slot> = Vec::new();
        for s in touched {
            let r = sh.rec(s);
            if r.flags.fetch_and(!super::F_GUESS, Ordering::AcqRel) & super::F_GUESS != 0 {
                // The guesses held to the end of the evaluation.
                sh.outstanding.fetch_sub(1, Ordering::Relaxed);
                rep.confirmations += 1;
                if r.flags.fetch_and(!super::F_PREDREAD, Ordering::AcqRel) & super::F_PREDREAD != 0 {
                    rep.predictions_confirmed += 1;
                }
            }
            let d = r.data();
            let snap = snaps.remove(&s).unwrap_or(super::Snap {
                done: false,
                sig: 0,
                out: None,
                binds: 0,
            });
            let st = r.st();
            let done = st != S_FREE && st != S_INERT && r.has(F_DONE) && d.res.is_some();
            if !done {
                continue;
            }
            let res = d.res.as_ref().expect("done");
            if !snap.done || snap.sig != res.sig {
                rep.nodes_run += 1;
                ran.push(s);
                if snap.out == Some(res.output) {
                    rep.cutoffs += 1;
                }
            }
            if snap.done && snap.binds != res.binds {
                rep.rebinds += 1;
            }
        }
        rep.pending_sets = rep.pending_waits;
        #[cfg(feature = "profile")]
        let th = std::time::Instant::now();
        self.update_heights(exec);
        #[cfg(feature = "profile")]
        #[allow(clippy::cast_possible_truncation)]
        crate::obs::add_impl("report_heights", th.elapsed().as_nanos() as u64);
        let path = self.heights.top();
        rep.critical_path = path;
        let hs = &self.heights.of;
        // Levels: the nodes that ran, by height; a level's width is how
        // many ran there (the steps a barrier schedule would take).
        let mut widths: FxHashMap<u32, u64> = FxHashMap::default();
        for s in ran {
            if let Some(Some(h)) = hs.get(s as usize) {
                *widths.entry(h.0).or_default() += 1;
            }
        }
        let mut buckets: Vec<u64> = Vec::new();
        for &w in widths.values() {
            let b = w.ilog2() as usize;
            if buckets.len() <= b {
                buckets.resize(b + 1, 0);
            }
            buckets[b] += 1;
            rep.max_ready = rep.max_ready.max(usize::try_from(w).unwrap_or(usize::MAX));
            rep.ready_total += w;
        }
        rep.steps = rep.steps.max(widths.len() as u64);
        rep.applied_widths = buckets;
        self.last_critical = path;
    }

    /// The critical chain from the kept heights: each node with its height,
    /// the cause linking it to the next, and the next.
    fn heights(&self) -> (u32, Vec<(Slot, u32, Cause, Slot)>) {
        let h = &self.heights;
        let sh = &self.sh;
        let live = |x: Slot| !matches!(sh.rec(x).st(), S_FREE | S_MARKER | S_INERT);
        if h.lev.iter().all(|&c| c == 0) {
            return (0, Vec::new());
        }
        let max = h.top();
        #[allow(clippy::cast_possible_truncation)]
        let top =
            h.of.iter()
                .enumerate()
                .filter(|(x, e)| e.as_ref().is_some_and(|e| e.0 == max) && live(*x as Slot))
                .map(|(x, _)| x as Slot)
                .min_by_key(|&x| sh.rec(x).lab())
                .unwrap_or(NIL);
        let mut chain = Vec::new();
        let mut x = top;
        let mut guard = 0;
        while x != NIL && guard < 1 << 20 {
            let Some((hx, c, nx)) = h.of.get(x as usize).cloned().flatten() else {
                break;
            };
            chain.push((x, hx, c, nx));
            x = nx;
            guard += 1;
        }
        (max, chain)
    }

    /// Brings the kept heights up to date from the nodes that changed
    /// since the last report (all of them the first time): a node's height
    /// is recomputed from its reads, and a change passes to its readers.
    fn update_heights(&mut self, exec: Option<&mut dyn crate::api::Executor>) {
        let sh = &self.sh;
        let mut seeds = std::mem::take(&mut *lock(&sh.hdirty));
        if !self.heights.init {
            self.heights.init = true;
            #[allow(clippy::cast_possible_truncation)]
            let nunits = sh.units.len() as u32;
            for u in 0..nunits {
                if sh.unit(u).live.load(Ordering::Acquire) {
                    seeds.extend(sh.entries(u));
                }
            }
        }
        let deps = |s: Slot| -> Vec<(Slot, Cause)> {
            #[cfg(feature = "profile")]
            let t0 = std::time::Instant::now();
            #[cfg(feature = "profile")]
            #[allow(clippy::cast_possible_truncation)]
            let _g = Tick(t0);
            let r = sh.rec(s);
            let d = r.data();
            let mut out = Vec::new();
            if let Some(res) = &d.res {
                for p in &res.log {
                    match p {
                        // A read of a prediction the definer bore out
                        // does not wait for it.
                        Pred::Def {
                            f,
                            spec: Some(q),
                            ans:
                                DAns::Site {
                                    node,
                                    val: super::DefVal::Val(x),
                                    ..
                                },
                            ..
                        } if q == node && sh.predicted(*q, *f) == Some(*x) => {}
                        Pred::Def {
                            f,
                            ans: DAns::Site { node, .. },
                            ..
                        } => out.push((
                            *node,
                            Cause::Changed {
                                site: Some(sh.node_id(*node)),
                                field: *f,
                            },
                        )),
                        Pred::Out { node, .. } | Pred::App { node, .. } => {
                            out.push((*node, Cause::Output { of: sh.node_id(*node) }));
                        }
                        Pred::Range { f, last, .. } if *last != NIL => out.push((
                            *last,
                            Cause::Range {
                                from: Some(sh.node_id(*last)),
                                field: *f,
                            },
                        )),
                        _ => {}
                    }
                }
                // Waits on pending sets order a read after a declaration
                // (other waits end on the site the read binds).
                for w in &res.waits {
                    let mut by = w.by;
                    if w.kind == "hint"
                        && (sh.rec(by).id.load(Ordering::Acquire) != w.id
                            || matches!(sh.rec(by).st(), S_FREE | S_MARKER | S_INERT))
                    {
                        // Gone: on from the hinted field's definer now.
                        let u = sh.unit_of(s);
                        match w.field.map(|f| sh.sread(u, || sh.q_def(s, f, false, s))) {
                            Some(super::query::Q::Ans(super::query::DefOut {
                                ans: DAns::Site { node, .. },
                                ..
                            })) => by = node,
                            _ => continue,
                        }
                    }
                    out.push((
                        by,
                        Cause::Woken {
                            by: NodeId(w.id),
                            field: w.field,
                            prefix: w.prefix.clone(),
                        },
                    ));
                }
            }
            if d.owner != NIL {
                out.push((
                    d.owner,
                    Cause::Created {
                        by: Some(sh.node_id(d.owner)),
                        from_hint: None,
                    },
                ));
            }
            out.retain(|(x, _)| *x != s && !matches!(sh.rec(*x).st(), S_FREE | S_MARKER | S_INERT));
            out
        };
        #[cfg(feature = "profile")]
        let mut tq = std::time::Instant::now();
        let h = &mut self.heights;
        let n = sh.recs.len();
        if h.of.len() < n {
            h.of.resize_with(n, || None);
            h.rev.resize_with(n, Vec::new);
        }
        let dead = |x: Slot| matches!(sh.rec(x).st(), S_FREE | S_MARKER | S_INERT);
        // The affected region: the changed nodes and every reader above
        // them; its heights are recomputed, the rest are kept.
        let mut in_region = vec![false; n];
        let mut region: Vec<Slot> = Vec::new();
        let mut work = seeds;
        while let Some(s) = work.pop() {
            let i = s as usize;
            if i < n && !in_region[i] {
                in_region[i] = true;
                region.push(s);
                work.extend(h.rev[i].iter().copied().filter(|&y| !in_region[y as usize]));
            }
        }
        for &s in &region {
            if let Some(o) = h.of[s as usize].take() {
                h.lev[o.0 as usize] -= 1;
            }
        }
        #[cfg(feature = "profile")]
        {
            #[allow(clippy::cast_possible_truncation)]
            crate::obs::add_impl("h_region", tq.elapsed().as_nanos() as u64);
            tq = std::time::Instant::now();
        }
        let mut order: Vec<Slot> = region.iter().copied().filter(|&x| !dead(x)).collect();
        order.sort_by_key(|&x| sh.rec(x).lab());
        for &x in &region {
            if dead(x) {
                h.rev[x as usize] = Vec::new();
            }
        }
        drop(region);
        #[cfg(feature = "profile")]
        {
            #[allow(clippy::cast_possible_truncation)]
            crate::obs::add_impl("h_sort", tq.elapsed().as_nanos() as u64);
            tq = std::time::Instant::now();
        }
        // The region's reads, gathered on the executor's threads when it
        // is large (the walk below takes them from here).
        let mut pre: Vec<Option<Vec<(Slot, Cause)>>> = Vec::new();
        if let Some(exec) = exec
            && exec.parallelism() > 1
            && order.len() >= 1 << 13
        {
            const CHUNK: usize = 1024;
            let chunks: Vec<&[Slot]> = order.chunks(CHUNK).collect();
            let out: Vec<std::sync::Mutex<Vec<NodeDeps>>> =
                chunks.iter().map(|_| std::sync::Mutex::new(Vec::new())).collect();
            exec.for_each(chunks.len(), &|i| {
                let v: Vec<NodeDeps> = chunks[i].iter().map(|&x| (x, deps(x))).collect();
                *lock(&out[i]) = v;
            });
            pre.resize_with(n, || None);
            for m in out {
                for (x, d) in m.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner) {
                    pre[x as usize] = Some(d);
                }
            }
        }
        #[cfg(feature = "profile")]
        {
            #[allow(clippy::cast_possible_truncation)]
            crate::obs::add_impl("h_pre", tq.elapsed().as_nanos() as u64);
            tq = std::time::Instant::now();
        }
        let mut deps_of = |x: Slot| {
            pre.get_mut(x as usize)
                .and_then(Option::take)
                .unwrap_or_else(|| deps(x))
        };
        // On the walk's stack (reads of a node on it are a cycle: skipped).
        let mut on = vec![false; n];
        let mut stack: Vec<Frame> = Vec::new();
        for &root in &order {
            if h.of[root as usize].is_some() {
                continue;
            }
            // Iterative DFS.
            stack.push((root, deps_of(root), 0));
            on[root as usize] = true;
            while let Some(top) = stack.last_mut() {
                let (s, ds, i) = (top.0, &top.1, top.2);
                if i < ds.len() {
                    let x = ds[i].0;
                    top.2 += 1;
                    if (x as usize) < n && h.of[x as usize].is_none() && !on[x as usize] {
                        on[x as usize] = true;
                        let dx = deps_of(x);
                        stack.push((x, dx, 0));
                    }
                    continue;
                }
                let mut best: (u32, Cause, Slot) = (
                    0,
                    Cause::Created {
                        by: None,
                        from_hint: None,
                    },
                    NIL,
                );
                let mut any = false;
                for (x, c) in ds {
                    let xi = *x as usize;
                    if xi >= n {
                        continue;
                    }
                    let rv = &mut h.rev[xi];
                    // Kept free of repeats (lists stay short; a long one is
                    // deduplicated when it doubles).
                    if rv.last() != Some(&s) && (rv.len() >= 16 || !rv.contains(&s)) {
                        rv.push(s);
                        if rv.len() >= 32 && rv.len().is_power_of_two() {
                            rv.sort_unstable();
                            rv.dedup();
                        }
                    }
                    let Some(hx) = h.of[xi].as_ref().map(|e| e.0) else {
                        continue;
                    };
                    let cand = hx + 1;
                    if !any || cand > best.0 || (cand == best.0 && rank(c) < rank(&best.1)) {
                        best = (cand, c.clone(), *x);
                        any = true;
                    }
                }
                if !any {
                    best.0 = 0;
                }
                on[s as usize] = false;
                let l = best.0 as usize;
                if h.lev.len() <= l {
                    h.lev.resize(l + 1, 0);
                }
                h.lev[l] += 1;
                h.of[s as usize] = Some(best);
                stack.pop();
            }
        }
        #[cfg(feature = "profile")]
        #[allow(clippy::cast_possible_truncation)]
        crate::obs::add_impl("h_walk", tq.elapsed().as_nanos() as u64);
    }

    /// The longest chain of reads below any node (as of the last
    /// evaluation's final state).
    #[must_use]
    pub fn critical_path(&self) -> u32 {
        self.heights().0
    }

    /// The critical chain: each node with the cause linking it to the next.
    #[must_use]
    pub fn critical_chain(&self) -> Vec<(NodeId, Cause)> {
        self.heights()
            .1
            .into_iter()
            .map(|(s, _, c, _)| (self.sh.node_id(s), c))
            .collect()
    }

    /// The critical chain with each node's height.
    #[must_use]
    pub fn critical_chain_heights(&self) -> Vec<(NodeId, u32, Cause)> {
        self.heights()
            .1
            .into_iter()
            .map(|(s, h, c, _)| (self.sh.node_id(s), h, c))
            .collect()
    }
}

#[cfg(feature = "profile")]
struct Tick(std::time::Instant);

#[cfg(feature = "profile")]
impl Drop for Tick {
    fn drop(&mut self) {
        #[allow(clippy::cast_possible_truncation)]
        crate::obs::add_impl("report_deps", self.0.elapsed().as_nanos() as u64);
    }
}
