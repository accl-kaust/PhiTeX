//! Waiters on a pending claim: `claimwait [chain] [followers] [threads]`.
//! `x0 .. x{chain}` is a chain (each reads the previous); `c` claims 0 and
//! is `Claim::Pending`, reading the chain's end; `followers` plain nodes
//! after it. Prints the waits by kind: each follower should wait on the
//! claim about once.

use std::sync::Arc;

use defuse::{Claim, Ctx, Engine, Field, Locality, OpId, Operation, Stop, ValueId, Values};

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "x" {
            vec![(v.field(&format!("x{}", v.as_int(a[1]).unwrap_or(0))), Locality::Local)]
        } else {
            Vec::new()
        })
    }
    fn claim(&self, a: &[ValueId], v: &Values, _f: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if tag(v, a) == "c" { Claim::Pending } else { Claim::None }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "x" => {
                let i = v.as_int(a[1]).unwrap_or(0);
                let p = if i == 0 {
                    0
                } else {
                    cx.use_field(v.field(&format!("x{}", i - 1)))?
                        .and_then(|x| v.as_int(x))
                        .unwrap_or(0)
                };
                let t = std::time::Instant::now();
                while t.elapsed() < std::time::Duration::from_micros(30) {
                    std::hint::spin_loop();
                }
                cx.define(v.field(&format!("x{i}")), v.int(p + 1));
                Ok(ValueId::UNIT)
            }
            "c" => {
                // A chain of claimers: each reads `x{k}` and follows with the
                // claimer of `x{k+1}` up to the chain's end.
                let n = v.as_int(a[1]).unwrap_or(0);
                let k = a.get(2).and_then(|x| v.as_int(*x)).unwrap_or(n);
                let e = cx.use_field(v.field(&format!("x{k}")))?;
                cx.claim(0);
                if k < n && std::env::var_os("CHAIN").is_some() {
                    cx.follow(cx.op(), vec![v.str("c"), a[1], v.int(k + 1)]);
                }
                Ok(e.unwrap_or(ValueId::UNIT))
            }
            _ => Ok(v.int(1)),
        }
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
    let a: Vec<i64> = std::env::args().skip(1).map(|s| s.parse().unwrap()).collect();
    let (chain, foll, th) = (
        a.first().copied().unwrap_or(50),
        a.get(1).copied().unwrap_or(200),
        a.get(2).copied().unwrap_or(8),
    );
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let s = |e: &Engine, t: &str| e.values().str(t);
    for i in 0..=chain {
        let iv = eng.values().int(i);
        eng.push(u, op, vec![s(&eng, "x"), iv]);
    }
    let nv = eng.values().int(chain);
    let k0 = eng
        .values()
        .int(if std::env::var_os("CHAIN").is_some() { 0 } else { chain });
    eng.push(u, op, vec![s(&eng, "c"), nv, k0]);
    for _ in 0..foll {
        eng.push(u, op, vec![s(&eng, "f")]);
    }
    let t = std::time::Instant::now();
    let rep = eng
        .evaluate(
            &mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256),
            &mut Threads(th as usize),
        )
        .unwrap();
    println!(
        "wall {:?} runs {} stops {} raises {} gated {} steps {} waits {:?}",
        t.elapsed(),
        rep.runs,
        rep.stops,
        rep.raises,
        rep.gated,
        rep.steps,
        rep.waits_by_kind
    );
}
