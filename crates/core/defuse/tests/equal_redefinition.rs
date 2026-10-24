//! A definition re-made at another node with an equal value leaves its
//! readers standing: `P [a]` follows `Q [a]`; `Q` defines `g = 7`; `R`
//! reads `g`. Editing `P` to `[b]` re-makes `Q [b]`, which defines `g = 7`.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use phitex_defuse::{Ctx, Engine, Field, Locality, OpId, Operation, Report, Stop, ValueId, Values};

struct Ops {
    me: Mutex<Option<OpId>>,
    reads: AtomicU64,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "q" {
            vec![(v.field("g"), Locality::Local)]
        } else {
            Vec::new()
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = self.me.lock().unwrap().unwrap();
        match tag(v, &a).as_str() {
            "p" => {
                cx.follow(op, vec![v.str("q"), a[1]]);
                Ok(ValueId::UNIT)
            }
            "q" => {
                let x = if v.as_str(a[1]).as_deref() == Some("c") { 8 } else { 7 };
                cx.define(v.field("g"), v.int(x));
                Ok(ValueId::UNIT)
            }
            _ => {
                self.reads.fetch_add(1, Relaxed);
                Ok(cx.use_field(v.field("g"))?.unwrap_or(ValueId::UNIT))
            }
        }
    }
}

#[test]
fn an_equal_value_from_a_remade_node_keeps_readers() {
    let ops = Arc::new(Ops {
        me: Mutex::new(None),
        reads: AtomicU64::new(0),
    });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    *ops.me.lock().unwrap() = Some(op);
    let u = eng.new_unit();
    let (p, a, r) = (eng.values().str("p"), eng.values().str("a"), eng.values().str("r"));
    let pn = eng.push(u, op, vec![p, a]);
    let rn = eng.push(u, op, vec![r]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(eng.output(rn), Some(eng.values().int(7)));
    let before = ops.reads.load(Relaxed);
    let b = eng.values().str("b");
    eng.replace(pn, op, vec![p, b]);
    let rep = eng.evaluate_sequential().unwrap();
    assert_eq!(eng.output(rn), Some(eng.values().int(7)));
    assert_eq!(ops.reads.load(Relaxed), before, "R stands: {:?}", rep.rerun_causes);
    // And it follows the new site: a later change there reaches it.
    let c = eng.values().str("c");
    eng.replace(pn, op, vec![p, c]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(
        eng.output(rn),
        Some(eng.values().int(8)),
        "a change at the new site reaches R"
    );
}

struct Threads;

impl phitex_defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        4
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
            for _ in 0..4 {
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

fn eval(eng: &mut Engine, sched: &str) -> Report {
    match sched {
        "seq" => eng.evaluate_sequential(),
        "spec" => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 256),
            &mut phitex_defuse::Sequential,
        ),
        "random" => eng.evaluate(&mut phitex_defuse::RandomScheduler::new(7, true), &mut phitex_defuse::Sequential),
        _ => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 256),
            &mut Threads,
        ),
    }
    .expect("evaluate")
}

/// The same under every schedule, with several readers after the re-made
/// definer (a reader may be checked while the new definer is in work).
#[test]
fn an_equal_value_from_a_remade_node_keeps_readers_in_every_schedule() {
    for sched in ["seq", "spec", "random", "par"] {
        for _ in 0..if sched == "par" { 50 } else { 1 } {
            let ops = Arc::new(Ops {
                me: Mutex::new(None),
                reads: AtomicU64::new(0),
            });
            let mut eng = Engine::new();
            let op = eng.register(ops.clone());
            *ops.me.lock().unwrap() = Some(op);
            let u = eng.new_unit();
            let (p, a, r) = (eng.values().str("p"), eng.values().str("a"), eng.values().str("r"));
            let pn = eng.push(u, op, vec![p, a]);
            let rs: Vec<_> = (0..8).map(|_| eng.push(u, op, vec![r])).collect();
            eval(&mut eng, sched);
            let before = ops.reads.load(Relaxed);
            let b = eng.values().str("b");
            eng.replace(pn, op, vec![p, b]);
            let rep = eval(&mut eng, sched);
            for &rn in &rs {
                assert_eq!(eng.output(rn), Some(eng.values().int(7)));
            }
            assert_eq!(
                ops.reads.load(Relaxed),
                before,
                "{sched}: readers stand: {:?}",
                rep.rerun_causes
            );
        }
    }
}
