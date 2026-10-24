//! A chain of N operations whose definition sets are unknown until they
//! run (`defs` is `None`, `may_define` is `Any`). Every operation does
//! `--work` microseconds of work and defines `x<i>`; most read only `b`,
//! defined before the chain, and the dependent ones (every `--every`th)
//! read `x<i-1>` too. The client's preamble load has this shape: under
//! speculation the independent operations should overlap, and a run whose
//! read values the committed results confirm should be kept.
//!
//! `cargo run --release --example specchain -- [--n 30] [--every 6] [--work 20000] [--threads 8]`

#![allow(
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{
    Ctx, Engine, Executor, Field, Greedy, Locality, Operation, RunResult, Speculation, Stop, Value, ValueId, Values,
};

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

static RUNS: AtomicU64 = AtomicU64::new(0);

struct Op {
    work: Duration,
}

fn int(v: &Values, a: &[ValueId], i: usize) -> i64 {
    match a.get(i).map(|&x| v.get(x)).as_deref() {
        Some(Value::Int(k)) => *k,
        _ => -1,
    }
}

impl Operation for Op {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        (int(v, a, 0) < 0).then(|| vec![(v.field("b"), Locality::Local)])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let i = int(v, &a, 0);
        if i < 0 {
            cx.define(v.field("b"), v.int(1));
            return Ok(ValueId::UNIT);
        }
        RUNS.fetch_add(1, Ordering::Relaxed);
        let b = cx.use_field(v.field("b"))?;
        let dep = int(v, &a, 1);
        let d = if dep >= 0 {
            cx.use_field(v.field(&format!("x{dep}")))?
        } else {
            None
        };
        let t = Instant::now();
        while t.elapsed() < self.work {
            std::hint::spin_loop();
        }
        let s = [b, d].into_iter().flatten().map(|x| match &*v.get(x) {
            Value::Int(k) => *k,
            _ => 0,
        });
        let out = v.int(s.sum::<i64>() + i);
        cx.define(v.field(&format!("x{i}")), out);
        Ok(out)
    }
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
    let (n, every, work, threads) = (
        opt("--n", 30),
        opt("--every", 6),
        opt("--work", 20000),
        opt("--threads", 8),
    );
    for spec in [false, true] {
        let mut eng = Engine::new();
        let op = eng.register(Arc::new(Op {
            work: Duration::from_micros(work),
        }));
        let u = eng.new_unit();
        let v = eng.values();
        let m1 = v.int(-1);
        eng.push(u, op, vec![m1]);
        for i in 0..n {
            let dep = if i > 0 && (i + 1) % every == 0 { i - 1 } else { u64::MAX };
            let dv = if dep == u64::MAX {
                m1
            } else {
                eng.values().int(dep as i64)
            };
            eng.push(u, op, vec![eng.values().int(i as i64), dv]);
        }
        RUNS.store(0, Ordering::Relaxed);
        phitex_defuse::obs::reset();
        let t = Instant::now();
        let r = if spec {
            eng.evaluate(
                &mut Greedy::speculative(Speculation::All, 64),
                &mut Threads(threads as usize),
            )
        } else {
            eng.evaluate(&mut Greedy::default(), &mut Threads(threads as usize))
        }
        .expect("evaluate");
        let wall = t.elapsed();
        println!(
            "{} wall {:.1} ms  op runs {}  (ideal {} = n)  speculations {} confirmations {} mispredictions {} spec_stops {} shape {} stops {} steps {}",
            if spec { "all " } else { "none" },
            wall.as_secs_f64() * 1e3,
            RUNS.load(Ordering::Relaxed),
            n,
            r.speculations,
            r.confirmations,
            r.mispredictions,
            r.spec_stops,
            r.spec_shape_changes,
            r.stops,
            r.steps,
        );
        if phitex_defuse::obs::enabled() {
            print!("{}", phitex_defuse::obs::snapshot());
        }
    }
}
