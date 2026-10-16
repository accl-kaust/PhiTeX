//! Declared claims and effects are guesses the run may refute: siblings
//! P, Q, R all declare `Claim::None` and `Effect::None`. P's run claims 1
//! (covering Q) and opens a frame of kind k: Q ends as no node of its own
//! (it may have run on the guess first), R runs inside P's frame and sees kind k. An edit making P
//! claim 0 and open nothing brings Q back, and R sees no frame. Every
//! result equals a cold build's, in every schedule.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use defuse::{Brackets, Ctx, Effect, Engine, Field, Locality, OpId, Operation, Report, Stop, ValueId, Values};

struct Ops {
    q_runs: AtomicU64,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "p" => {
                let on = v.as_int(a[1]).unwrap_or(0) != 0;
                if on {
                    cx.claim(1);
                    cx.set_effect(Effect::Scope(Brackets {
                        opens: 1,
                        kind: Some(v.int(7)),
                        ..Brackets::default()
                    }));
                }
                Ok(ValueId::UNIT)
            }
            "q" => {
                self.q_runs.fetch_add(1, Relaxed);
                Ok(v.int(1))
            }
            _ => Ok(cx.scope_kind()?.unwrap_or(v.int(-1))),
        }
    }
}
struct Threads;

impl defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        4
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

fn eval(eng: &mut Engine, i: usize) -> Report {
    match i % 4 {
        0 => eng.evaluate_sequential(),
        1 => eng.evaluate(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
            &mut defuse::Sequential,
        ),
        2 => eng.evaluate(
            &mut defuse::RandomScheduler::new(i as u64, true),
            &mut defuse::Sequential,
        ),
        _ => eng.evaluate(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
            &mut Threads,
        ),
    }
    .expect("evaluate")
}

struct Built {
    eng: Engine,
    ops: Arc<Ops>,
    op: OpId,
    p: defuse::NodeId,
    q: defuse::NodeId,
    r: defuse::NodeId,
}

fn build(on: i64) -> Built {
    let ops = Arc::new(Ops {
        q_runs: AtomicU64::new(0),
    });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    let u = eng.new_unit();
    let s = |e: &Engine, t: &str| e.values().str(t);
    let k = eng.values().int(on);
    let p = eng.push(u, op, vec![s(&eng, "p"), k]);
    let q = eng.push(u, op, vec![s(&eng, "q")]);
    let r = eng.push(u, op, vec![s(&eng, "r")]);
    Built { eng, ops, op, p, q, r }
}

fn seen(b: &Built) -> (Option<i64>, Option<i64>) {
    let int = |n| b.eng.output(n).and_then(|x| b.eng.values().as_int(x));
    (int(b.q), int(b.r))
}

#[test]
fn a_run_refutes_its_declared_claim_and_effect() {
    for i in 0..80 {
        let mut b = build(1);
        let rep = eval(&mut b.eng, i);
        // (Q may have run on the guess before P's commit: a wasted run, not
        // a result.)
        let _ = b.ops.q_runs.load(Relaxed);
        assert_eq!(seen(&b).0, None, "Q claimed: no node output ({i})");
        assert_eq!(seen(&b).1, Some(7), "R inside P's frame ({i})");
        assert!(rep.claims_refuted >= 1 && rep.effects_refuted >= 1, "{rep:?}");
        let mut cold = build(1);
        eval(&mut cold.eng, 0);
        assert_eq!(seen(&b), seen(&cold));
        // P claims nothing and opens nothing now: Q is back, R outside.
        let (pv, z) = (b.eng.values().str("p"), b.eng.values().int(0));
        b.eng.replace(b.p, b.op, vec![pv, z]);
        eval(&mut b.eng, i + 1);
        assert_eq!(seen(&b), (Some(1), Some(-1)), "edit {i}");
        let mut cold = build(0);
        eval(&mut cold.eng, 0);
        assert_eq!(seen(&b), seen(&cold), "edit = cold {i}");
        // And back again.
        let one = b.eng.values().int(1);
        b.eng.replace(b.p, b.op, vec![pv, one]);
        eval(&mut b.eng, i + 2);
        assert_eq!(seen(&b).1, Some(7), "re-edit {i}");
    }
}
