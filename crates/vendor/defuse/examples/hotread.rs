//! Many readers of one field: `hotread [n] [threads]`. Node `k` reads `s`
//! and defines `x{k}`; every 5000th node defines `s` too. Prints the cold
//! wall and the commit time per run.
use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops;

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let k = v.as_int(a[0]).unwrap_or(0);
        let mut d = vec![(v.field(&format!("x{k}")), Locality::Local)];
        if k % 5000 == 0 {
            d.push((v.field("s"), Locality::Local));
        }
        Some(d)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let k = v.as_int(cx.args()[0]).unwrap_or(0);
        let s = cx.use_field(v.field("s"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
        if k % 5000 == 0 {
            cx.define(v.field("s"), v.int(k));
        }
        cx.define(v.field(&format!("x{k}")), v.int(s + k));
        Ok(ValueId::UNIT)
    }
}

struct Threads(usize);

impl defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0
    }
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> defuse::RunResult + Sync),
        out: &mut Vec<defuse::RunResult>,
    ) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<std::sync::OnceLock<defuse::RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..self.0 {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        let _ = slots[i].set(run(i));
                    }
                });
            }
        });
        out.extend(slots.into_iter().map(|s| s.into_inner().expect("ran")));
    }
}

fn main() {
    let a: Vec<usize> = std::env::args().skip(1).map(|s| s.parse().unwrap()).collect();
    let n = a.first().copied().unwrap_or(50_000);
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    for k in 0..n {
        let kv = eng.values().int(k as i64);
        eng.push(u, op, vec![kv]);
    }
    let t = std::time::Instant::now();
    let th = a.get(1).copied().unwrap_or(1);
    let r = if th == 1 {
        eng.evaluate_sequential().unwrap()
    } else {
        eng.evaluate(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
            &mut Threads(th),
        )
        .unwrap()
    };
    let commit: u64 = r.round_times.iter().map(|t| t.commit_ns).sum();
    if defuse::obs::enabled() {
        print!("{}", defuse::obs::snapshot());
    }
    println!(
        "n {n}: wall {:?} runs {} commit {:.1} ms ({:.2} us/run)",
        t.elapsed(),
        r.runs,
        commit as f64 / 1e6,
        commit as f64 / 1e3 / r.runs as f64
    );
}
