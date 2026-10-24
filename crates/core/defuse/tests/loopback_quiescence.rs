//! A loop-back value reaches its readers only once the round that
//! publishes it has quiesced. Finishers `F_i` each read the previous `d`
//! and predict `d = 100`, which never holds, so their runs are refuted;
//! the last one publishes loop-back field `lb` (initially 0) from its `d`.
//! Readers before and after the chain record every value of `lb` they
//! read. The publication does not depend on `lb`: readers must see 0 in
//! the first round and the final value after, never a value a refuted run
//! published on the way.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops {
    seen: Mutex<Vec<i64>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "f" | "p" => vec![(v.field("d"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn predicted_defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        matches!(tag(v, a).as_str(), "f" | "p").then(|| vec![(v.field("d"), v.int(100))])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            t @ ("f" | "p") => {
                let prev = cx.use_field(v.field("d"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                let d = v.int(prev + v.as_int(a[1]).unwrap_or(0) + 1);
                cx.define(v.field("d"), d);
                if t == "p" {
                    cx.publish(v.field("lb"), d);
                }
                Ok(d)
            }
            _ => {
                let x = cx.use_field(v.field("lb"))?;
                let n = x.and_then(|x| v.as_int(x)).unwrap_or(-1);
                self.seen.lock().unwrap().push(n);
                Ok(x.unwrap_or(ValueId::UNIT))
            }
        }
    }
}

struct Threads(usize);

impl phitex_defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0
    }
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> phitex_defuse::RunResult + Sync),
        out: &mut Vec<phitex_defuse::RunResult>,
    ) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<std::sync::OnceLock<phitex_defuse::RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..self.0 {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Relaxed);
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

const N: i64 = 30;

/// The chain's published value: d after N finishers and the publisher.
fn truth() -> i64 {
    (0..=N).map(|i| i + 1).sum()
}

#[test]
fn readers_see_only_quiesced_loopback_values() {
    let mut refuted = 0;
    for i in 0..60 {
        let ops = Arc::new(Ops {
            seen: Mutex::new(Vec::new()),
        });
        let mut eng = Engine::new();
        let op = eng.register(ops.clone());
        let u = eng.new_unit();
        let (lb, zero) = (eng.values().field("lb"), eng.values().int(0));
        eng.declare_loopback(lb, zero);
        let r = eng.values().str("r");
        for _ in 0..5 {
            eng.push(u, op, vec![r]);
        }
        for k in 0..N {
            let (f, x) = (eng.values().str("f"), eng.values().int(k));
            eng.push(u, op, vec![f, x]);
        }
        let (p, x) = (eng.values().str("p"), eng.values().int(N));
        eng.push(u, op, vec![p, x]);
        for _ in 0..5 {
            eng.push(u, op, vec![r]);
        }
        let spec = if i % 2 == 0 {
            phitex_defuse::Speculation::Predicted
        } else {
            phitex_defuse::Speculation::All
        };
        let rep = eng
            .evaluate(&mut phitex_defuse::Greedy::speculative(spec, 256), &mut Threads(1 + i % 6))
            .expect("evaluate");
        refuted += rep.predictions_refuted + rep.mispredictions;
        let seen = ops.seen.lock().unwrap().clone();
        let want = truth();
        assert!(
            seen.iter().all(|&x| x == 0 || x == want),
            "schedule {i}: readers saw {seen:?}; only 0 and {want} were published at quiescence"
        );
        assert!(
            seen.contains(&want),
            "schedule {i}: the final value never reached a reader: {seen:?}"
        );
    }
    // (The schedules did refute runs on the way: transient publications
    // were there to leak.)
    assert!(refuted > 0, "no run was refuted: the test proves nothing");
}
