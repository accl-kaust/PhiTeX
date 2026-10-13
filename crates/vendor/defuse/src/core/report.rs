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
    of: FxHashMap<Slot, (u32, Cause, Slot)>,
    lev: std::collections::BTreeMap<u32, crate::values::FxHashSet<Slot>>,
    rev: FxHashMap<Slot, Vec<Slot>>,
}

/// A node on the height walk's stack: its reads and how many are done.
type Frame = (Slot, Vec<(Slot, Cause)>, usize);

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
    pub(crate) fn fill_report(&mut self, rep: &mut Report) {
        let sh = &self.sh;
        let c = &sh.c;
        rep.runs = c.runs.load(Ordering::Relaxed);
        rep.stops = c.stops.load(Ordering::Relaxed);
        rep.raises = c.raises.load(Ordering::Relaxed);
        rep.pending_waits = c.pending_waits.load(Ordering::Relaxed);
        rep.pending_sets_defining = c.pending_defining.load(Ordering::Relaxed);
        rep.speculations = c.speculations.load(Ordering::Relaxed);
        rep.confirmations = c.confirmations.load(Ordering::Relaxed);
        rep.mispredictions = c.mispredictions.load(Ordering::Relaxed);
        rep.fuel_outs = c.fuel_outs.load(Ordering::Relaxed);
        rep.spec_stops = c.spec_stops.load(Ordering::Relaxed);
        rep.resumed = c.resumed.load(Ordering::Relaxed);
        rep.restarted = c.restarted.load(Ordering::Relaxed);
        rep.declarations_refuted = c.declarations_refuted.load(Ordering::Relaxed);
        rep.env_provided = c.env_provided.load(Ordering::Relaxed);
        rep.scope_region = c.scope_region.load(Ordering::Relaxed);
        rep.gated = c.gated.load(Ordering::Relaxed);
        rep.created = c.created.load(Ordering::Relaxed);
        rep.freed = c.freed.load(Ordering::Relaxed);
        {
            let t = lock(&sh.t);
            rep.runs_by_op = sorted(&t.runs_by_op);
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
        let touched = std::mem::take(&mut *lock(&sh.touched));
        let mut ran: Vec<Slot> = Vec::new();
        let mut seen = crate::values::FxHashSet::default();
        for s in touched {
            if !seen.insert(s) {
                continue;
            }
            let r = sh.rec(s);
            if r.flags.fetch_and(!super::F_GUESS, Ordering::AcqRel) & super::F_GUESS != 0 {
                // The guesses held to the end of the evaluation.
                sh.outstanding.fetch_sub(1, Ordering::Relaxed);
                rep.confirmations += 1;
            }
            let d = r.data();
            let Some(snap) = d.snap else { continue };
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
        self.update_heights();
        let path = self.heights.lev.keys().next_back().copied().unwrap_or(0);
        rep.critical_path = path;
        let hs = &self.heights.of;
        // Levels: the nodes that ran, by height; a level's width is how
        // many ran there (the steps a barrier schedule would take).
        let mut widths: FxHashMap<u32, u64> = FxHashMap::default();
        for s in ran {
            if let Some(h) = hs.get(&s) {
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
        let Some((&max, set)) = h.lev.iter().next_back() else {
            return (0, Vec::new());
        };
        let top = set
            .iter()
            .copied()
            .filter(|&x| live(x))
            .min_by_key(|&x| sh.rec(x).lab())
            .unwrap_or(NIL);
        let mut chain = Vec::new();
        let mut x = top;
        let mut guard = 0;
        while x != NIL && guard < 1 << 20 {
            let Some((hx, c, nx)) = h.of.get(&x).cloned() else {
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
    fn update_heights(&mut self) {
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
            let r = sh.rec(s);
            let d = r.data();
            let mut out = Vec::new();
            if let Some(res) = &d.res {
                for p in &res.log {
                    match p {
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
                for w in res.waits.iter().filter(|w| w.kind == "pending set") {
                    out.push((
                        w.by,
                        Cause::Woken {
                            by: sh.node_id(w.by),
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
        let h = &mut self.heights;
        // The affected region: the changed nodes and every reader above
        // them; its heights are recomputed, the rest are kept.
        let mut region: crate::values::FxHashSet<Slot> = crate::values::FxHashSet::default();
        let mut work = seeds;
        while let Some(s) = work.pop() {
            if region.insert(s)
                && let Some(rv) = h.rev.get(&s)
            {
                work.extend(rv.iter().copied().filter(|y| !region.contains(y)));
            }
        }
        for &s in &region {
            if let Some(o) = h.of.remove(&s)
                && let Some(set) = h.lev.get_mut(&o.0)
            {
                set.remove(&s);
                if set.is_empty() {
                    h.lev.remove(&o.0);
                }
            }
        }
        let mut order: Vec<Slot> = region
            .iter()
            .copied()
            .filter(|&x| !matches!(sh.rec(x).st(), S_FREE | S_MARKER | S_INERT))
            .collect();
        order.sort_by_key(|&x| sh.rec(x).lab());
        for &x in &region {
            if matches!(sh.rec(x).st(), S_FREE | S_MARKER | S_INERT) {
                h.rev.remove(&x);
            }
        }
        for &root in &order {
            if h.of.contains_key(&root) {
                continue;
            }
            // Iterative DFS.
            let mut stack: Vec<Frame> = vec![(root, deps(root), 0)];
            let mut on: crate::values::FxHashSet<Slot> = crate::values::FxHashSet::default();
            on.insert(root);
            while let Some(top) = stack.last_mut() {
                let (s, ds, i) = (top.0, &top.1, top.2);
                if i < ds.len() {
                    let x = ds[i].0;
                    top.2 += 1;
                    if !h.of.contains_key(&x) && !on.contains(&x) {
                        on.insert(x);
                        let dx = deps(x);
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
                    let rv = h.rev.entry(*x).or_default();
                    if !rv.contains(&s) {
                        rv.push(s);
                    }
                    let Some(hx) = h.of.get(x).map(|e| e.0) else {
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
                on.remove(&s);
                h.lev.entry(best.0).or_default().insert(s);
                h.of.insert(s, best);
                stack.pop();
            }
        }
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
