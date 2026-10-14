//! Static contributions ([`Operation::static_contribs`]): 1000 nodes in a
//! chain (each reads `x` and redefines it), each contributing its index to
//! the accumulator `acc`; every 10th node instead reads the fold of `acc`
//! over its predecessors. With the contributions declared, the readers do
//! not wait for the chain. Links also contribute 1 to `b` without
//! declaring it: the last node, reading `b`, waits for every run. One node can declare a wrong contribution
//! (`--wrong I`): its run refutes it and the readers after it run again.
//!
//! `cargo run --release --example staticacc -- [--n 1000] [--wrong 500]`

#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use defuse::{
    Ctx, Engine, Executor, Field, Greedy, Locality, NodeId, Operation, RandomScheduler, RunResult, Sequential,
    Speculation, Stop, SumMonoid, Value, ValueId, Values,
};

/// `[i]`: a reader when `i % 10 == 9`, else a chain link contributing `i`.
struct Op {
    /// Declare the contributions (else the readers wait for the chain).
    declare: bool,
    /// The node whose declaration is off by 1000 (`u64::MAX`: none).
    wrong: u64,
    /// Links read the `x` the previous link defined (else they only
    /// define it).
    chain: bool,
}

/// Reader attempts (runs and stops).
static READS: AtomicU64 = AtomicU64::new(0);

fn index(v: &Values, a: &[ValueId]) -> u64 {
    match a.first().map(|&x| v.get(x)).as_deref() {
        Some(Value::Int(k)) => *k as u64,
        _ => 0,
    }
}

impl Operation for Op {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if index(v, a) % 10 == 9 || a.len() > 1 {
            Vec::new()
        } else {
            vec![(v.field("x"), Locality::Local)]
        })
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        if !self.declare {
            return None;
        }
        let i = index(v, a);
        if i % 10 == 9 || a.len() > 1 {
            // Contributes nothing: the empty value.
            return Some(vec![(v.field("acc"), v.int(0))]);
        }
        let k = if i == self.wrong { i + 1000 } else { i };
        Some(vec![(v.field("acc"), v.int(k as i64))])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let i = index(v, &a);
        if a.len() > 1 {
            // The reader of `b`, which only runs contribute to.
            return cx.acc_range(v.field("b"), None);
        }
        if i % 10 == 9 {
            READS.fetch_add(1, Ordering::Relaxed);
            return cx.acc_range_to(v.field("acc"), None, None);
        }
        let prev = if self.chain { cx.use_field(v.field("x"))? } else { None };
        let s = prev.and_then(|p| v.as_int(p)).unwrap_or(0);
        cx.define(v.field("x"), v.int(s + 1));
        cx.contribute(v.field("acc"), v.int(i as i64));
        // Undeclared: readers of `b` wait for the run.
        cx.contribute(v.field("b"), v.int(1));
        Ok(ValueId::UNIT)
    }
}

/// Every job on its own claim of `T` scoped threads, however few the
/// jobs (each run here is long).
struct Threads(usize);

impl Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0.max(1)
    }
    fn execute(&mut self, n: usize, run: &(dyn Fn(usize) -> RunResult + Sync), out: &mut Vec<RunResult>) {
        let next = AtomicU64::new(0);
        let slots: Vec<std::sync::OnceLock<RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..self.0.clamp(1, n.max(1)) {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed) as usize;
                        if i >= n {
                            break;
                        }
                        let _ = slots[i].set(run(i));
                    }
                });
            }
        });
        out.extend(slots.into_iter().map(|s| s.into_inner().expect("every job ran")));
    }
}

#[derive(Clone, Copy, Debug)]
enum Sched {
    Seq,
    Random,
    Spec,
    Par,
}

fn build(n: u64, chain: bool, declare: bool, wrong: u64) -> (Engine, Vec<NodeId>) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Op { declare, wrong, chain }));
    let acc = eng.values().field("acc");
    eng.declare_accumulator(acc, Arc::new(SumMonoid));
    let b = eng.values().field("b");
    eng.declare_accumulator(b, Arc::new(SumMonoid));
    let u = eng.new_unit();
    let mut ids: Vec<NodeId> = (0..n)
        .map(|i| {
            let a = vec![eng.values().int(i as i64)];
            eng.push(u, op, a)
        })
        .collect();
    let a = vec![eng.values().int(n as i64), eng.values().str("b")];
    ids.push(eng.push(u, op, a));
    (eng, ids)
}

fn eval(eng: &mut Engine, s: Sched) -> defuse::Report {
    match s {
        Sched::Seq => eng.evaluate_sequential(),
        Sched::Random => eng.evaluate(&mut RandomScheduler::new(7, true), &mut Sequential),
        Sched::Spec => eng.evaluate(&mut Greedy::speculative(Speculation::All, 512), &mut Sequential),
        Sched::Par => eng.evaluate(&mut Greedy::default(), &mut Threads(4)),
    }
    .expect("evaluate")
}

fn outputs(eng: &Engine, ids: &[NodeId]) -> Vec<Option<i64>> {
    let v = eng.values();
    ids.iter().map(|&id| eng.output(id).and_then(|x| v.as_int(x))).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let opt = |name: &str, d: u64| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(d)
    };
    let (n, wrong) = (opt("--n", 1000), opt("--wrong", 500));
    for chain in [false, true] {
        println!(
            "== {n} nodes, links {}",
            if chain { "in a chain" } else { "independent" }
        );
        let (mut cold, cids) = build(n, chain, false, u64::MAX);
        READS.store(0, Ordering::Relaxed);
        let r = eval(&mut cold, Sched::Par);
        let want = outputs(&cold, &cids);
        println!(
            "undeclared     par    critical path {:>4}  runs {:>5}  reader attempts {:>4}",
            r.critical_path,
            r.runs,
            READS.load(Ordering::Relaxed)
        );
        for s in [Sched::Seq, Sched::Random, Sched::Spec, Sched::Par] {
            for w in [u64::MAX, wrong] {
                let (mut eng, ids) = build(n, chain, true, w);
                READS.store(0, Ordering::Relaxed);
                let r = eval(&mut eng, s);
                let got = outputs(&eng, &ids);
                println!(
                    "declared {:<5} {:<6} critical path {:>4}  runs {:>5}  reader attempts {:>4}  refuted {}  same as undeclared: {}",
                    if w == u64::MAX { "right" } else { "wrong" },
                    format!("{s:?}").to_lowercase(),
                    r.critical_path,
                    r.runs,
                    READS.load(Ordering::Relaxed),
                    r.static_contribs_refuted,
                    got == want
                );
                assert_eq!(got, want, "{s:?} wrong={w}");
            }
        }
    }
}
