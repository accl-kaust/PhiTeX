//! The profile's report and snapshot, and tier 2 (DESIGN 7.21): `tune`,
//! run by the client when the scheduler is idle, never by `run`.
//!
//! Every decision keeps the results: a memoized op returns what it
//! returned before for the same operands (it is pure, and its key is its
//! operands' versions), and sealing is exact (7.11). What changes is
//! only time and memory.

use super::{Graph, Kind};
use crate::lang::Lang;
use crate::profile::{LEAF, OpStats, RegionStats, Report, SCAN, Snapshot, Tuned};

impl<L: Lang, const P: bool> Graph<L, P> {
    /// The quiet time of the step at `si` before it is sealed again.
    pub(super) fn quiet_of(&self, si: usize) -> u32 {
        if self.quiet.is_empty() {
            return self.cfg.seal_quiet;
        }
        self.quiet
            .get(&self.steps[si].key)
            .copied()
            .unwrap_or(self.cfg.seal_quiet)
    }

    /// Tags filled in, a loaded snapshot's stats matched, the store's
    /// probes and hits read.
    fn prof_settle(&mut self) {
        self.prof.flush();
        let per_tag = self
            .hook
            .memo
            .lock()
            .expect("the memo store")
            .per_tag
            .clone();
        let prior = &mut self.prof.prior;
        for (op, st) in &mut self.prof.ops {
            if st.tag == 0 {
                st.tag = L::op_tag(*op);
                if let Some(p) = prior.remove(&st.tag) {
                    let memo = p.memo;
                    st.add(&OpStats {
                        memo_probes: 0,
                        memo_hits: 0,
                        memo: false,
                        ..p
                    });
                    // (the earlier session's decision, kept as a wish)
                    st.memo |= memo;
                }
            }
            if let Some(&(p, h)) = per_tag.get(&st.tag) {
                st.memo_probes = p;
                st.memo_hits = h;
            }
        }
    }

    /// The profile so far (`Graph<L, true>`; empty otherwise): ops by
    /// estimated total time, the regions that re-ran most.
    pub fn profile(&mut self) -> Report<L::Op> {
        self.prof_settle();
        let mut ops: Vec<(L::Op, OpStats)> =
            self.prof.ops.iter().map(|(o, s)| (*o, s.clone())).collect();
        ops.sort_by_key(|(_, s)| (std::cmp::Reverse(s.total_ns()), std::cmp::Reverse(s.steps)));
        let mut regions: Vec<(u64, RegionStats)> =
            self.prof.regions.iter().map(|(k, r)| (*k, *r)).collect();
        regions.sort_by_key(|(k, r)| (std::cmp::Reverse(r.runs), *k));
        let regions_seen = regions.len();
        regions.truncate(64);
        Report {
            ops,
            regions,
            regions_seen,
            runs: self.prof.runs,
        }
    }

    /// The profile in a form a later session loads (`load_profile`).
    pub fn snapshot(&mut self) -> Snapshot {
        self.prof_settle();
        let mut ops: Vec<OpStats> = self.prof.ops.values().cloned().collect();
        ops.extend(self.prof.prior.values().cloned());
        ops.sort_by_key(|s| s.tag);
        let mut regions: Vec<(u64, RegionStats)> =
            self.prof.regions.iter().map(|(k, r)| (*k, *r)).collect();
        regions.sort_unstable_by_key(|r| r.0);
        Snapshot {
            ops,
            regions,
            runs: self.prof.runs,
        }
    }

    /// An earlier session's profile, added to this one's: its ops count
    /// once they are seen here (by tag), its regions at once, and its
    /// memo decisions are applied by the next `tune`.
    pub fn load_profile(&mut self, s: &Snapshot) {
        for o in &s.ops {
            self.prof.prior.entry(o.tag).or_default().add(o);
            self.prof.prior.get_mut(&o.tag).expect("just made").tag = o.tag;
        }
        for (k, r) in &s.regions {
            let e = self.prof.regions.entry(*k).or_default();
            e.runs += r.runs;
        }
        self.prof.runs += s.runs;
    }

    /// Tier 2: the profile acted on. Call it when the scheduler is idle
    /// (no edit waiting); with work queued it does nothing (`busy`). It
    /// opts ops in to the memo store when their cost times their reuse
    /// is above a probe's cost, and out when their hits stay low; it
    /// starts measuring the reuse of ops that cost more than a probe;
    /// and it gives hot regions a longer quiet time before sealing and
    /// quiet ones a shorter, sealing what is now due.
    #[allow(clippy::cast_precision_loss, reason = "statistics")]
    pub fn tune(&mut self) -> Tuned {
        let mut t = Tuned::default();
        if !P {
            return t;
        }
        if !self.heap.is_empty() {
            t.busy = true;
            return t;
        }
        self.prof_settle();
        let probe = self.cfg.memo_probe_ns as f64;
        let epoch = self.epoch;
        let min_s = self.cfg.tune_min_samples;
        let min_k = self.cfg.tune_min_keyed;
        let ops: Vec<(L::Op, OpStats)> =
            self.prof.ops.iter().map(|(o, s)| (*o, s.clone())).collect();
        for (op, st) in ops {
            if st.kind & (LEAF | SCAN) == 0 || st.impure || L::memo(op) {
                continue;
            }
            let cost = st.mean_ns();
            let wish = st.memo && !self.hook.auto.has(&op);
            if self.hook.auto.has(&op) {
                let (p0, h0) = self.prof.since.get(&op).copied().unwrap_or((0, 0));
                let (dp, dh) = (st.memo_probes - p0, st.memo_hits - h0);
                if dp >= min_k && st.timed >= min_s && cost * (dh as f64 / dp as f64) < probe {
                    self.hook.auto.remove(&op);
                    self.prof.veto.insert(op, epoch + 64);
                    self.prof.ops.get_mut(&op).expect("seen").memo = false;
                    t.memo_out.push(st.tag);
                }
                continue;
            }
            if self.prof.veto.get(&op).is_some_and(|&v| v > epoch) {
                continue;
            }
            let reuse = if st.keyed > 0 {
                st.repeats as f64 / st.keyed as f64
            } else {
                0.0
            };
            let measured = st.keyed >= min_k && st.timed >= min_s;
            if wish || (measured && cost * reuse > probe) {
                self.hook.auto.insert(op);
                self.hook.watch.remove(&op);
                self.prof.unwatch(&op);
                self.prof.since.insert(op, (st.memo_probes, st.memo_hits));
                self.prof.ops.get_mut(&op).expect("seen").memo = true;
                t.memo_in.push(st.tag);
            } else if self.hook.watch.has(&op) {
                if measured && cost * reuse < probe / 2.0 && st.keyed >= 4 * min_k {
                    // (not worth measuring further for a while)
                    self.hook.watch.remove(&op);
                    self.prof.unwatch(&op);
                    self.prof.veto.insert(op, epoch + 64);
                }
            } else if st.timed >= min_s && cost >= probe {
                self.hook.watch.insert(op);
                t.watched.push(st.tag);
            }
        }
        if self.hook.auto.len() > 0 && !self.hook.memo_on {
            self.set_memo_budget(self.cfg.auto_memo_bytes);
        }
        if self.cfg.seal > 0 {
            self.tune_sealing(&mut t);
        }
        t
    }

    /// Quiet times per region from the re-run profile, the hot queue
    /// re-ordered by them, and what is due sealed now.
    fn tune_sealing(&mut self, t: &mut Tuned) {
        let q = self.cfg.seal_quiet;
        let epoch = self.epoch;
        self.quiet.clear();
        for (k, r) in &self.prof.regions {
            if r.runs >= self.cfg.hot_runs && epoch - r.last.min(epoch) <= 4 * q {
                self.quiet.insert(*k, q.saturating_mul(4));
                t.hot += 1;
            } else if r.runs <= 1 {
                self.quiet.insert(*k, (q / 8).max(1));
                t.quiet += 1;
            }
        }
        let mut hot: Vec<(u32, u32)> = std::mem::take(&mut self.hot)
            .into_iter()
            .filter_map(|(due, s)| {
                let h = &self.n.h[s as usize];
                if h.kind != Kind::Step || self.n.is_dead(s) {
                    return None;
                }
                let si = h.aux as usize;
                let ran = self.steps[si].ran;
                Some((
                    if ran == 0 {
                        due
                    } else {
                        ran + self.quiet_of(si)
                    },
                    s,
                ))
            })
            .collect();
        hot.sort_by_key(|x| x.0);
        hot.dedup_by_key(|x| x.1);
        self.hot = hot.into();
        let before = self.rep.sealed;
        // (the first build sealed everything it could at its end)
        if self.epoch > 1 {
            self.seal_due_now();
            // (the registries pruned, what sealing freed reused: as at a
            // run's end)
            self.tidy();
        }
        t.sealed = self.rep.sealed - before;
    }
}
