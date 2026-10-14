//! The value-table mix of the client's workload: N independent
//! operations, each interning `--interns` values (names from a shared
//! pool, which hit, and one fresh list per ten, which insert) and reading
//! `--reads` values from a pool larger than a thread's read cache. Prints
//! the job time per operation (summed inside the runs) and the result
//! fingerprint, for one process with T workers; run 12 processes at
//! `--threads 1 --part k/12` to compare with separate processes.
//!
//! `cargo run --release --example valuemix -- [--n 6591] [--threads T] [--part k/m]`

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use defuse::{Ctx, Engine, Executor, Field, Greedy, Locality, Operation, RunResult, Stop, Value, ValueId, Values};

static JOB_NS: AtomicU64 = AtomicU64::new(0);

struct Threads(usize);

impl Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0.max(1)
    }
    fn execute(&mut self, n: usize, run: &(dyn Fn(usize) -> RunResult + Sync), out: &mut Vec<RunResult>) {
        if self.0 <= 1 {
            out.extend((0..n).map(run));
            return;
        }
        let next = AtomicU64::new(0);
        let slots: Vec<std::sync::OnceLock<RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..self.0.min(n.max(1)) {
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

struct Op {
    interns: u64,
    reads: u64,
    pool: Arc<Vec<ValueId>>,
}

impl Operation for Op {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let t = Instant::now();
        let v = cx.values();
        let a = cx.args();
        let i = match &*v.get(a[0]) {
            Value::Int(k) => *k as u64,
            _ => 0,
        };
        let mut acc = 0u64;
        let mut x = i.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        for j in 0..self.interns {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let id = if j % 10 == 0 {
                v.list(vec![a[0], v.int(j as i64)])
            } else {
                v.str(&format!("name{}", x % 2000))
            };
            acc = acc.wrapping_add(id.raw());
        }
        for _ in 0..self.reads {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let id = self.pool[(x % self.pool.len() as u64) as usize];
            if let Value::Int(k) = &*v.get(id) {
                acc = acc.wrapping_add(*k as u64);
            }
        }
        let out = v.int(acc as i64);
        JOB_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        Ok(out)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let n: u64 = opt("--n").and_then(|s| s.parse().ok()).unwrap_or(6591);
    let threads: usize = opt("--threads").and_then(|s| s.parse().ok()).unwrap_or(1);
    let interns: u64 = opt("--interns").and_then(|s| s.parse().ok()).unwrap_or(218);
    let reads: u64 = opt("--reads").and_then(|s| s.parse().ok()).unwrap_or(190);
    let (k, m) = opt("--part")
        .and_then(|s| {
            s.split_once('/')
                .map(|(a, b)| (a.parse().unwrap_or(0), b.parse().unwrap_or(1)))
        })
        .unwrap_or((0u64, 1u64));
    let mut eng = Engine::new();
    let pool: Vec<ValueId> = (0..200_000).map(|i| eng.values().int(1_000_000 + i)).collect();
    let op = eng.register(Arc::new(Op {
        interns,
        reads,
        pool: Arc::new(pool),
    }));
    let u = eng.new_unit();
    let ids: Vec<_> = (0..n)
        .filter(|i| i % m == k)
        .map(|i| {
            let a = vec![eng.values().int(i as i64)];
            eng.push(u, op, a)
        })
        .collect();
    let t = Instant::now();
    eng.evaluate(&mut Greedy::default(), &mut Threads(threads))
        .expect("evaluate");
    let wall = t.elapsed();
    let fp = ids.iter().fold(0u64, |h, &id| {
        h.wrapping_mul(31).wrapping_add(eng.output(id).map_or(0, ValueId::raw))
    });
    let ops = ids.len() as f64;
    println!(
        "ops {} threads {threads} wall {:.1} ms job per op {:.1} us fingerprint {fp:x}",
        ids.len(),
        wall.as_secs_f64() * 1e3,
        JOB_NS.load(Ordering::Relaxed) as f64 / ops / 1e3
    );
}
