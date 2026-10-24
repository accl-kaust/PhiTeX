//! Pending definition sets: a unit of N nodes, each one's definitions
//! unknown until it runs (`defs` is `None`: it may define anything), node
//! `i` defining `x<i>` and reading `x<i-1>`, `x<i-2>` and `x<i-3>` (the
//! `--spread S` option reads `x<i-1>`, `x<i-S>`, `x<i-2S>` instead). Every
//! read may be hidden by any node between the definer and the reader, so a
//! read waits on the nearest node with a pending set.
//!
//! Prints, per schedule, the time, runs, stops, pending waits and the
//! cost of a stop against a completed run.
//!
//! `--follow`: the chain shape instead: a first node and a "rest of the
//! text" node pushed, every node's claim pending until it runs; each node
//! reads three names each defined by a pseudo-random earlier node, then
//! claims the rest node and leaves the next node and a new rest node as
//! its continuations (`Ctx::follow`).
//!
//! `cargo run --release --example pending -- [N..] [--threads T] [--spread S] [--only SCHEDULE] [--follow]`

#![allow(clippy::cast_precision_loss, clippy::many_single_char_names)]

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::Threads;
use phitex_defuse::{
    Claim, Ctx, Engine, Field, Greedy, Locality, OpId, Operation, Sequential, Speculation, Stop, Value, ValueId, Values,
};

struct Node {
    spread: usize,
    follow: bool,
}

/// Runs of command nodes and of rest nodes.
static RUNS: [std::sync::atomic::AtomicU64; 2] = [
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
];

/// A pseudo-random earlier node for the `k`th read of node `i`.
fn earlier(i: usize, k: u64) -> usize {
    let h = (i as u64)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(k.wrapping_mul(0xbf58_476d_1ce4_e5b9));
    let h = (h ^ (h >> 31)).wrapping_mul(0x94d0_49bb_1331_11eb);
    usize::try_from((h ^ (h >> 29)) % i as u64).unwrap_or(0)
}

impl Operation for Node {
    fn name(&self) -> &'static str {
        "node"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn claim(&self, _: &[ValueId], _: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if self.follow { Claim::Pending } else { Claim::None }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let int = |k: usize| match a.get(k).map(|&x| v.get(x)).as_deref() {
            Some(Value::Int(n)) => usize::try_from(*n).unwrap_or(0),
            _ => 0,
        };
        let (i, n) = (int(0), int(1));
        RUNS[usize::from(a.len() > 2)].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // The rest node: nothing until the command before it runs.
        if a.len() > 2 {
            if self.follow {
                cx.claim(0);
            }
            return Ok(ValueId::UNIT);
        }
        let mut sum: i64 = 0;
        if self.follow {
            for k in 0..3 {
                if i > 0 {
                    let f = v.field(&format!("x{}", earlier(i, k)));
                    sum += cx.use_field(f)?.and_then(|y| v.as_int(y)).unwrap_or(0);
                }
            }
        } else {
            for k in [1, self.spread, 2 * self.spread] {
                if i >= k {
                    let f = v.field(&format!("x{}", i - k));
                    sum += cx.use_field(f)?.and_then(|y| v.as_int(y)).unwrap_or(0);
                }
            }
        }
        cx.define(v.field(&format!("x{i}")), v.int(sum % 1000 + 1));
        if self.follow {
            // Takes the rest node; leaves the next command and a new rest.
            let has_rest = cx.following(0).is_some();
            cx.claim(usize::from(has_rest));
            if i + 1 < n {
                let op = cx.op();
                let (ni, nn) = (
                    v.int(i64::try_from(i + 1).unwrap_or(0)),
                    v.int(i64::try_from(n).unwrap_or(0)),
                );
                cx.follow(op, vec![ni, nn]);
                cx.follow(op, vec![ni, nn, v.str("rest")]);
            }
        }
        Ok(ValueId::UNIT)
    }
}

/// A schedule to time, by name.
type Schedule = (&'static str, Box<dyn Fn(&mut Engine) -> phitex_defuse::Report>);

fn build(n: usize, spread: usize, follow: bool) -> (Engine, OpId) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Node { spread, follow }));
    let u = eng.new_unit();
    let total = i64::try_from(n).unwrap_or(0);
    for i in 0..if follow { 1 } else { n } {
        let v = eng.values();
        let a = vec![v.int(i64::try_from(i).unwrap_or(0)), v.int(total)];
        eng.push(u, op, a);
    }
    if follow {
        let v = eng.values();
        let a = vec![v.int(0), v.int(total), v.str("rest")];
        eng.push(u, op, a);
    }
    (eng, op)
}

fn main() {
    let mut sizes = Vec::new();
    let mut threads = 16;
    let mut spread = 1;
    let mut only: Option<String> = None;
    let mut follow = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--threads" => threads = args.next().and_then(|s| s.parse().ok()).expect("--threads T"),
            "--spread" => spread = args.next().and_then(|s| s.parse().ok()).expect("--spread S"),
            "--only" => only = args.next(),
            "--follow" => follow = true,
            s => sizes.push(s.parse::<usize>().expect("size")),
        }
    }
    if sizes.is_empty() {
        sizes = vec![10_000, 131_000];
    }
    let spread = spread.max(1);
    println!(
        "{:>8} {:>12} {:>9} {:>9} {:>9} {:>9} {:>9} {:>8}",
        "nodes", "schedule", "time", "runs", "stops", "pending", "critical", "stop/run"
    );
    for &n in &sizes {
        let shapes: Vec<Schedule> = vec![
            (
                "sequential",
                Box::new(|e: &mut Engine| e.evaluate_sequential().expect("sequential")),
            ),
            (
                "greedy/1",
                Box::new(|e: &mut Engine| {
                    e.evaluate(&mut Greedy::speculative(Speculation::Off, 256), &mut Sequential)
                        .expect("greedy")
                }),
            ),
            (
                "spec/1",
                Box::new(|e: &mut Engine| {
                    e.evaluate(&mut Greedy::speculative(Speculation::All, 256), &mut Sequential)
                        .expect("spec")
                }),
            ),
            (
                "greedy/T",
                Box::new(move |e: &mut Engine| {
                    e.evaluate(&mut Greedy::speculative(Speculation::Off, 256), &mut Threads(threads))
                        .expect("greedy threads")
                }),
            ),
            (
                "spec/T",
                Box::new(move |e: &mut Engine| {
                    e.evaluate(&mut Greedy::speculative(Speculation::All, 256), &mut Threads(threads))
                        .expect("spec threads")
                }),
            ),
        ];
        for (name, eval) in &shapes {
            if only.as_deref().is_some_and(|o| o != *name) {
                continue;
            }
            let (mut eng, _) = build(n, spread, follow);
            if let Some(k) = std::env::var("PENDING_LIMIT").ok().and_then(|s| s.parse().ok()) {
                let mut l = eng.limits().clone();
                l.runs_per_node = k;
                eng.set_limits(l);
            }
            let t0 = Instant::now();
            let r = eval(&mut eng);
            let t = t0.elapsed();
            println!(
                "{n:>8} {name:>12} {:>8.3}s {:>9} {:>9} {:>9} {:>9} {:>8.2}",
                t.as_secs_f64(),
                r.runs,
                r.stops,
                r.pending_waits,
                r.critical_path,
                r.stops as f64 / r.runs.max(1) as f64
            );
            if follow {
                println!(
                    "         command runs {}  rest runs {}",
                    RUNS[0].swap(0, std::sync::atomic::Ordering::Relaxed),
                    RUNS[1].swap(0, std::sync::atomic::Ordering::Relaxed)
                );
            }
            if r.stops > 0 {
                println!(
                    "         waits by kind {:?}  top waited {:?}  raises {}",
                    r.waits_by_kind, r.top_waited, r.raises
                );
            }
        }
    }
}
