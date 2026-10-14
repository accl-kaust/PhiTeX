//! A chain of N operations, each reading the two fields its predecessor
//! defined and defining two of its own, with `--work` microseconds of work.
//! Without predictions the chain runs one node after another. With
//! [`Operation::predicted_defs`] answering each node's values (as a
//! client's record of an earlier run would), readers run on the predictions
//! and the chain overlaps; a wrong prediction (`--wrong k`) re-runs only
//! what read it.
//!
//! `cargo run --release --example predchain -- [--n 50] [--work 2000] [--threads 12] [--wrong 25]`

#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use defuse::{
    Ctx, Engine, Executor, Field, Greedy, Locality, NodeId, Operation, RunResult, Speculation, Stop, Value, ValueId,
    Values,
};

/// Every job on its own thread of `T` scoped threads, however few the jobs.
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

static RUNS: AtomicU64 = AtomicU64::new(0);

/// The values node `i` defines: `(a, b)` from its predecessor's.
fn step(i: i64, (a, b): (i64, i64)) -> (i64, i64) {
    ((a + b + i) % 1_000_003, (a * 3 + i) % 1_000_033)
}

fn truth(n: usize) -> Vec<(i64, i64)> {
    let mut out = Vec::with_capacity(n);
    let mut p = (1, 2);
    for i in 0..n {
        p = step(i as i64, p);
        out.push(p);
    }
    out
}

/// `[i]`: node `i` of the chain.
struct Chain {
    work: Duration,
    /// Predicted `(a, b)` per node (`None`: no predictions).
    predict: Option<Vec<(i64, i64)>>,
}

fn idx(v: &Values, a: &[ValueId]) -> usize {
    match &*v.get(a[0]) {
        Value::Int(k) => *k as usize,
        _ => 0,
    }
}

fn fields(v: &Values, i: usize) -> (Field, Field) {
    (v.field(&format!("a{i}")), v.field(&format!("b{i}")))
}

impl Operation for Chain {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let (fa, fb) = fields(v, idx(v, a));
        Some(vec![(fa, Locality::Local), (fb, Locality::Local)])
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        let i = idx(v, a);
        Some(if i == 0 {
            Vec::new()
        } else {
            let (fa, fb) = fields(v, i - 1);
            vec![fa, fb]
        })
    }
    fn predicted_defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        let i = idx(v, a);
        let &(pa, pb) = self.predict.as_ref()?.get(i)?;
        let (fa, fb) = fields(v, i);
        Some(vec![(fa, v.int(pa)), (fb, v.int(pb))])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let i = idx(v, &a);
        let prev = if i == 0 {
            (1, 2)
        } else {
            let (fa, fb) = fields(v, i - 1);
            let get = |x: Option<ValueId>| x.and_then(|x| v.as_int(x)).unwrap_or(0);
            let pa = get(cx.use_field(fa)?);
            let pb = get(cx.use_field(fb)?);
            (pa, pb)
        };
        RUNS.fetch_add(1, Ordering::Relaxed);
        let t = Instant::now();
        while t.elapsed() < self.work {
            std::hint::spin_loop();
        }
        let (na, nb) = step(i as i64, prev);
        let (fa, fb) = fields(v, i);
        cx.define(fa, v.int(na));
        cx.define(fb, v.int(nb));
        Ok(v.int(na))
    }
}

/// Builds the chain and evaluates it; returns the outputs, the report and
/// the wall time.
fn build(n: usize, work: Duration, predict: Option<Vec<(i64, i64)>>) -> (Engine, Vec<NodeId>) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Chain { work, predict }));
    let u = eng.new_unit();
    let ids = (0..n)
        .map(|i| {
            let a = vec![eng.values().int(i as i64)];
            eng.push(u, op, a)
        })
        .collect();
    (eng, ids)
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
    let (n, work, threads) = (
        opt("--n", 50) as usize,
        opt("--work", 2000),
        opt("--threads", 12) as usize,
    );
    let wrong = opt("--wrong", u64::MAX);
    let policy = if args.iter().any(|a| a == "--predicted") {
        Speculation::Predicted
    } else {
        Speculation::All
    };
    let work = Duration::from_micros(work);
    let mut reference = None;
    for (label, predict) in [
        ("none", None),
        ("right", Some(truth(n))),
        (
            "wrong",
            Some({
                let mut p = truth(n);
                if let Some(x) = p.get_mut(wrong as usize) {
                    x.0 += 1;
                }
                p
            }),
        ),
    ] {
        if label == "wrong" && wrong == u64::MAX {
            continue;
        }
        for t in [1, threads] {
            let (mut eng, ids) = build(n, work, predict.clone());
            RUNS.store(0, Ordering::Relaxed);
            let start = Instant::now();
            let r = eng
                .evaluate(&mut Greedy::speculative(policy, 256), &mut Threads(t))
                .expect("evaluate");
            let wall = start.elapsed();
            let outs: Vec<Option<ValueId>> = ids.iter().map(|&i| eng.output(i)).collect();
            let ints: Vec<Option<i64>> = outs.iter().map(|o| o.and_then(|x| eng.values().as_int(x))).collect();
            let same = match &reference {
                None => {
                    reference = Some(ints.clone());
                    true
                }
                Some(r) => *r == ints,
            };
            println!(
                "{label:5} threads {t:2}  wall {:7.1} ms  runs {:3}  critical path {:2}  realised {:2}  steps {:2}  \
                 speculations {} confirmed {} refuted {} mispredictions {}  same output {same}",
                wall.as_secs_f64() * 1e3,
                RUNS.load(Ordering::Relaxed),
                r.critical_path,
                r.realised_critical_path,
                r.steps,
                r.speculations,
                r.predictions_confirmed,
                r.predictions_refuted,
                r.mispredictions,
            );
            if std::env::var_os("CAUSES").is_some() {
                for (id, why) in &r.rerun_causes {
                    println!("  rerun {id:?} {why:?}");
                }
            }
            assert!(same, "output differs from the run without predictions");
        }
    }
}
