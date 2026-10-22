//! A loop-back declared by a run, and published in that run, moves with
//! the round values that name it: a reader never sees the order (a field
//! declared before, published by the same run) name a unit while the
//! unit reads its initial value, with early fields in the evaluation or
//! without.

use std::sync::{Arc, Mutex};

use defuse::{Ctx, Engine, Field, Locality, LoopOptions, Operation, Stop, ValueId, Values};

struct Ops {
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl Operation for Ops {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let (order, early) = (v.field("order"), v.field("early"));
        match v.as_str(a[0]).as_deref() {
            // The files node: declares the unit, publishes it and the order.
            // (One unit more each round, as `rounds` moves: the order names
            // them all.)
            Some("files") => {
                let k = cx.use_field(v.field("rounds"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                // (Driven by the writers' early field too, as PhiTeX's is.)
                let k = k.max(cx.use_field(v.field("wdef"))?.and_then(|x| v.as_int(x)).unwrap_or(0).min(3));
                let mut names = Vec::new();
                for i in 0..=k {
                    let n = format!("unit:{i}");
                    let u = v.field(&n);
                    cx.declare_loopback_with(u, v.str(""), LoopOptions::default());
                    cx.publish(u, v.str("content"));
                    names.push(n);
                }
                cx.publish(order, v.str(&names.join(" ")));
                cx.publish(v.field("rounds"), v.int((k + 1).min(3)));
            }
            // A reader of the order and the units it names.
            Some("reader") => {
                let o = cx.use_field(order)?.and_then(|x| v.as_str(x)).unwrap_or_default();
                for n in o.split(' ').filter(|n| !n.is_empty()) {
                    let got = cx.use_field(v.field(n))?.and_then(|x| v.as_str(x)).unwrap_or_else(|| "none".into());
                    self.seen.lock().unwrap().push((n.to_string(), got));
                }
            }
            // A writer: publishes the early, last-wins `wdef` from what the
            // reader saw (the order's length).
            Some("writer") => {
                let o = cx.use_field(order)?.and_then(|x| v.as_str(x)).unwrap_or_default();
                let n = o.split(' ').filter(|n| !n.is_empty()).count();
                cx.publish(v.field("wdef"), v.int(i64::try_from(n).unwrap_or(0)));
            }
            Some("pub_early") => {
                let n = cx.use_field(early)?.and_then(|x| v.as_int(x)).unwrap_or(0);
                cx.publish(early, v.int(n.min(2) + u64::from(n < 2) as i64));
            }
            _ => {}
        }
        Ok(ValueId::UNIT)
    }
}

struct Threads(usize);

impl defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0
    }
    fn execute(&mut self, n: usize, run: &(dyn Fn(usize) -> defuse::RunResult + Sync), out: &mut Vec<defuse::RunResult>) {
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

fn seen_with(early: bool, threads: usize) -> Vec<(String, String)> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops { seen: seen.clone() }));
    let (order, e) = (eng.values().field("order"), eng.values().field("early"));
    let blank = eng.values().str("");
    eng.declare_loopback(order, blank);
    let (rounds, zero) = (eng.values().field("rounds"), eng.values().int(0));
    eng.declare_loopback(rounds, zero);
    if early {
        let zero = eng.values().int(0);
        eng.declare_loopback_early(e, zero);
        let wdef = eng.values().field("wdef");
        eng.declare_loopback_with(wdef, zero, LoopOptions { early: true, last_wins: true });
    }
    let u = eng.new_unit();
    for t in ["reader", "writer", "files", "pub_early", "reader", "writer"] {
        let a = vec![eng.values().str(t)];
        eng.push(u, op, a);
    }
    if threads == 1 {
        eng.evaluate_sequential().expect("evaluate");
    } else {
        eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 64), &mut Threads(threads)).expect("evaluate");
    }
    let s = seen.lock().unwrap().clone();
    s
}

#[test]
fn a_run_declared_unit_moves_with_the_order_naming_it() {
    for early in [false, true] {
        for threads in [1, 4] {
            let s = seen_with(early, threads);
            assert!(!s.iter().any(|(_, u)| u.is_empty()), "early {early} threads {threads}: a unit the order names seen at its initial value: {s:?}");
            assert!(s.contains(&("unit:3".to_string(), "content".to_string())), "early {early} threads {threads}: {s:?}");
        }
    }
}
