//! Profiles (DESIGN 7.21): what each op cost and how often it ran, what
//! the memo store and CSE did for it, and how often each region re-ran
//! on edits. A `Graph<L, true>` records them; `Graph<L>` (the default,
//! `P = false`) compiles them out. `Graph::tune`, run when the scheduler
//! is idle, reads them to opt ops in to the memo store and out again,
//! and to seal quiet regions sooner and keep hot ones live.
//!
//! Leaf and scan evaluations are sampled (one in `SAMPLE` on average, at
//! random intervals): a sample is timed and stands for the evaluations
//! of its interval, so the counts are estimates and the means are
//! unbiased. Steps, regions, memo and CSE probes are counted exactly.

use crate::graph::{Map, Set};
use crate::ver::Ver;

/// Evaluations a sample stands for, on average.
pub const SAMPLE: u32 = 128;

/// What kind of node an op was seen on (bits: it may be several).
pub const LEAF: u8 = 1;
pub const SCAN: u8 = 2;
pub const STEP: u8 = 4;

/// One op's profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpStats {
    /// The client's stable identity of the op (`Lang::op_tag`).
    pub tag: u64,
    /// `LEAF`, `SCAN`, `STEP`, or'd.
    pub kind: u8,
    /// Evaluations (leaves, scan elements): estimated from the samples.
    pub evals: u64,
    /// Evaluations timed (memo hits not counted), and their total ns.
    pub timed: u64,
    pub time_ns: u64,
    /// Steps run (an unfold's op), and those run on an edit (after the
    /// first build).
    pub steps: u64,
    pub reruns: u64,
    /// Memo store probes and hits.
    pub memo_probes: u64,
    pub memo_hits: u64,
    /// CSE probes and merges.
    pub cse_probes: u64,
    pub cse_hits: u64,
    /// Evaluations whose key was looked at (watched ops), and those whose
    /// key had been seen before: the reuse a memo would find.
    pub keyed: u64,
    pub repeats: u64,
    /// Emitted at least once with a class other than `Pure`: never
    /// memoized automatically.
    pub impure: bool,
    /// Memoized by the auto opt-in.
    pub memo: bool,
}

impl OpStats {
    /// Mean cost of one evaluation (ns), from the timed samples.
    #[must_use]
    #[allow(clippy::cast_precision_loss, reason = "a statistic")]
    pub fn mean_ns(&self) -> f64 {
        if self.timed == 0 {
            0.0
        } else {
            self.time_ns as f64 / self.timed as f64
        }
    }

    /// The fraction of evaluations a memo would answer: its hit rate
    /// where it was on, else the repeats among the keyed evaluations.
    #[must_use]
    #[allow(clippy::cast_precision_loss, reason = "a statistic")]
    pub fn reuse(&self) -> f64 {
        if self.memo_probes > 0 {
            self.memo_hits as f64 / self.memo_probes as f64
        } else if self.keyed > 0 {
            self.repeats as f64 / self.keyed as f64
        } else {
            0.0
        }
    }

    /// Estimated total time spent in the op (ns).
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a statistic"
    )]
    pub fn total_ns(&self) -> u64 {
        (self.mean_ns() * self.evals as f64) as u64
    }

    /// Another profile of the same op added in.
    pub fn add(&mut self, o: &OpStats) {
        self.kind |= o.kind;
        self.evals += o.evals;
        self.timed += o.timed;
        self.time_ns += o.time_ns;
        self.steps += o.steps;
        self.reruns += o.reruns;
        self.memo_probes += o.memo_probes;
        self.memo_hits += o.memo_hits;
        self.cse_probes += o.cse_probes;
        self.cse_hits += o.cse_hits;
        self.keyed += o.keyed;
        self.repeats += o.repeats;
        self.impure |= o.impure;
        self.memo |= o.memo;
    }
}

/// A region's profile: a step of a root unfold (or the sealed run it is
/// in), by its key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegionStats {
    /// Runs on an edit (after the first build).
    pub runs: u32,
    /// The run (`Graph` epoch) it last ran in.
    pub last: u32,
}

/// `Graph::profile`: the ops by estimated total time, and the regions
/// that re-ran most.
#[derive(Clone, Debug)]
pub struct Report<O> {
    pub ops: Vec<(O, OpStats)>,
    /// The 64 regions that re-ran most: (key, stats).
    pub regions: Vec<(u64, RegionStats)>,
    /// Regions seen re-running at all.
    pub regions_seen: usize,
    /// Runs (`Graph::run`) profiled.
    pub runs: u32,
}

/// A profile in a form a later session can load (`Graph::load_profile`):
/// ops by their stable tag.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub ops: Vec<OpStats>,
    pub regions: Vec<(u64, RegionStats)>,
    pub runs: u32,
}

/// An error reading a snapshot.
#[derive(Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// Not a snapshot, or of another format version.
    Format,
    /// Cut short.
    Truncated,
}

const MAGIC: &[u8; 8] = b"phiprof1";

impl Snapshot {
    /// The bytes: magic, then little-endian fields.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.ops.len() * 112 + self.regions.len() * 16);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.runs.to_le_bytes());
        out.extend_from_slice(&(self.ops.len() as u64).to_le_bytes());
        for o in &self.ops {
            for x in [
                o.tag,
                o.evals,
                o.timed,
                o.time_ns,
                o.steps,
                o.reruns,
                o.memo_probes,
                o.memo_hits,
                o.cse_probes,
                o.cse_hits,
                o.keyed,
                o.repeats,
            ] {
                out.extend_from_slice(&x.to_le_bytes());
            }
            out.extend_from_slice(&[o.kind, u8::from(o.impure), u8::from(o.memo)]);
        }
        out.extend_from_slice(&(self.regions.len() as u64).to_le_bytes());
        for (k, r) in &self.regions {
            out.extend_from_slice(&k.to_le_bytes());
            out.extend_from_slice(&r.runs.to_le_bytes());
            out.extend_from_slice(&r.last.to_le_bytes());
        }
        out
    }

    /// A snapshot from its bytes.
    ///
    /// # Errors
    ///
    /// A wrong magic or version, or bytes cut short.
    pub fn from_bytes(mut b: &[u8]) -> Result<Snapshot, SnapshotError> {
        fn take<'a>(b: &mut &'a [u8], n: usize) -> Result<&'a [u8], SnapshotError> {
            let (h, t) = b.split_at_checked(n).ok_or(SnapshotError::Truncated)?;
            *b = t;
            Ok(h)
        }
        fn u64_(b: &mut &[u8]) -> Result<u64, SnapshotError> {
            Ok(u64::from_le_bytes(take(b, 8)?.try_into().expect("8 bytes")))
        }
        fn u32_(b: &mut &[u8]) -> Result<u32, SnapshotError> {
            Ok(u32::from_le_bytes(take(b, 4)?.try_into().expect("4 bytes")))
        }
        if take(&mut b, 8).map_err(|_| SnapshotError::Format)? != MAGIC {
            return Err(SnapshotError::Format);
        }
        let runs = u32_(&mut b)?;
        let n = u64_(&mut b)?;
        let mut ops = Vec::new();
        for _ in 0..n {
            let mut f = [0u64; 12];
            for x in &mut f {
                *x = u64_(&mut b)?;
            }
            let t = take(&mut b, 3)?;
            ops.push(OpStats {
                tag: f[0],
                evals: f[1],
                timed: f[2],
                time_ns: f[3],
                steps: f[4],
                reruns: f[5],
                memo_probes: f[6],
                memo_hits: f[7],
                cse_probes: f[8],
                cse_hits: f[9],
                keyed: f[10],
                repeats: f[11],
                kind: t[0],
                impure: t[1] != 0,
                memo: t[2] != 0,
            });
        }
        let n = u64_(&mut b)?;
        let mut regions = Vec::new();
        for _ in 0..n {
            let k = u64_(&mut b)?;
            let runs = u32_(&mut b)?;
            let last = u32_(&mut b)?;
            regions.push((k, RegionStats { runs, last }));
        }
        Ok(Snapshot { ops, regions, runs })
    }
}

/// What `Graph::tune` did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tuned {
    /// Work was queued: nothing was done (tune only when idle).
    pub busy: bool,
    /// Ops (by tag) opted in to the memo store, and out of it.
    pub memo_in: Vec<u64>,
    pub memo_out: Vec<u64>,
    /// Ops (by tag) whose reuse is now measured.
    pub watched: Vec<u64>,
    /// Regions given a shorter quiet time (sealed sooner), and a longer
    /// one (kept live).
    pub quiet: usize,
    pub hot: usize,
    /// Steps sealed by this call.
    pub sealed: u64,
}

/// A sampled or keyed evaluation, as a step records it (folded into the
/// profile after the step).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ev<O> {
    pub op: O,
    /// Evaluations the sample stands for (0: keyed only, not sampled).
    pub w: u32,
    /// Its time (ns), if timed (not a memo hit).
    pub ns: Option<u32>,
    /// Its key, if the op is watched.
    pub key: Ver,
    pub kind: u8,
}

/// Keys a watched op keeps (then it starts again: repeats further apart
/// are not seen, which only makes the reuse estimate low).
const KEYS: usize = 1 << 14;

/// The profile as the graph keeps it: by op value (its tag is filled in
/// when it is reported or saved).
pub(crate) struct Prof<O> {
    pub(crate) ops: Map<O, OpStats>,
    pub(crate) regions: Map<u64, RegionStats>,
    /// Watched ops' recent keys.
    keys: Map<O, Set<u128>>,
    /// A loaded snapshot's ops, by tag, not yet matched to a live op.
    pub(crate) prior: Map<u64, OpStats>,
    /// Runs profiled.
    pub(crate) runs: u32,
    /// Memo probes and hits of an auto-memoized op when it was opted in
    /// (its rate is judged on what came after), and the run until which
    /// an op opted out stays out.
    pub(crate) since: Map<O, (u64, u64)>,
    pub(crate) veto: Map<O, u32>,
    /// Steps of one op in a row, counted before they are added.
    acc: Option<(O, u64, u64)>,
}

impl<O> Default for Prof<O> {
    fn default() -> Self {
        Prof {
            ops: Map::default(),
            regions: Map::default(),
            keys: Map::default(),
            prior: Map::default(),
            runs: 0,
            since: Map::default(),
            veto: Map::default(),
            acc: None,
        }
    }
}

impl<O: std::hash::Hash + Eq + Copy> Prof<O> {
    pub(crate) fn fold(&mut self, ev: Ev<O>) {
        let e = self.ops.entry(ev.op).or_default();
        e.kind |= ev.kind;
        e.evals += u64::from(ev.w);
        if let Some(ns) = ev.ns {
            e.timed += 1;
            e.time_ns += u64::from(ns);
        }
        if ev.key != Ver::ABSENT {
            e.keyed += 1;
            let ks = self.keys.entry(ev.op).or_default();
            if !ks.insert(ev.key.0) {
                e.repeats += 1;
            }
            if ks.len() >= KEYS {
                ks.clear();
            }
        }
    }

    pub(crate) fn cse(&mut self, op: O, hit: bool) {
        let e = self.ops.entry(op).or_default();
        e.cse_probes += 1;
        e.cse_hits += u64::from(hit);
    }

    pub(crate) fn impure(&mut self, op: O) {
        self.ops.entry(op).or_default().impure = true;
    }

    /// A step of `op` ran (on an edit: `rerun`).
    #[inline]
    pub(crate) fn step(&mut self, op: O, rerun: bool) {
        match &mut self.acc {
            Some((o, n, r)) if *o == op => {
                *n += 1;
                *r += u64::from(rerun);
            }
            _ => {
                self.flush();
                self.acc = Some((op, 1, u64::from(rerun)));
            }
        }
    }

    /// The steps counted in a row, added.
    pub(crate) fn flush(&mut self) {
        if let Some((op, n, r)) = self.acc.take() {
            let e = self.ops.entry(op).or_default();
            e.kind |= STEP;
            e.steps += n;
            e.reruns += r;
        }
    }

    pub(crate) fn region(&mut self, key: u64, epoch: u32) {
        let r = self.regions.entry(key).or_default();
        r.runs += 1;
        r.last = epoch;
    }

    /// A segment's profile added in.
    pub(crate) fn merge(&mut self, mut o: Prof<O>) {
        o.flush();
        for (op, st) in o.ops {
            self.ops.entry(op).or_default().add(&st);
        }
        for (k, r) in o.regions {
            let e = self.regions.entry(k).or_default();
            e.runs += r.runs;
            e.last = e.last.max(r.last);
        }
    }

    pub(crate) fn unwatch(&mut self, op: &O) {
        self.keys.remove(op);
    }
}

/// The sampling countdown: random intervals, mean `SAMPLE`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tick {
    left: u32,
    last: u32,
    rng: u64,
}

impl Default for Tick {
    fn default() -> Self {
        Tick {
            left: SAMPLE,
            last: SAMPLE,
            rng: 0x9e37_79b9_7f4a_7c15,
        }
    }
}

impl Tick {
    /// Counts an evaluation that is not a sample, or says it is one
    /// (then `next` draws the next interval).
    #[inline]
    pub(crate) fn due(&mut self) -> bool {
        if self.left > 1 {
            self.left -= 1;
            false
        } else {
            true
        }
    }

    /// Whether this evaluation is a sample: the evaluations it stands for
    /// (0: not a sample).
    #[inline]
    pub(crate) fn next(&mut self) -> u32 {
        self.left -= 1;
        if self.left > 0 {
            return 0;
        }
        let w = self.last;
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        #[allow(clippy::cast_possible_truncation, reason = "below 2 SAMPLE")]
        let next = 1 + (self.rng % u64::from(2 * SAMPLE - 1)) as u32;
        self.last = next;
        self.left = next;
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_round_trips_and_a_bad_one_errs() {
        let s = Snapshot {
            ops: vec![OpStats {
                tag: 7,
                kind: LEAF | SCAN,
                evals: 100,
                timed: 3,
                time_ns: 900,
                memo: true,
                ..OpStats::default()
            }],
            regions: vec![(42, RegionStats { runs: 5, last: 9 })],
            runs: 12,
        };
        let b = s.to_bytes();
        assert_eq!(Snapshot::from_bytes(&b), Ok(s));
        assert_eq!(
            Snapshot::from_bytes(&b[..b.len() - 1]),
            Err(SnapshotError::Truncated)
        );
        assert_eq!(
            Snapshot::from_bytes(b"nonsense"),
            Err(SnapshotError::Format)
        );
    }

    #[test]
    fn samples_stand_for_their_intervals() {
        let mut t = Tick::default();
        let n = 1_000_000u64;
        let w: u64 = (0..n).map(|_| u64::from(t.next())).sum();
        // (the weights add up to the evaluations, but the last interval)
        assert!(w <= n && n - w < u64::from(2 * SAMPLE), "{w}");
    }
}
