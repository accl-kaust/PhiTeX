//! Predicted definitions across re-commits: finishers `F_i` define `d`
//! from the environment's `base` (plus i), predicting `d = 100`; readers
//! `R_i` after each read `d`. An edit of `base` makes every finisher
//! re-commit a value other than its prediction: no committed read may be
//! left on the old value (`check_reads` empty) and the outputs equal a
//! cold build's, in every schedule including predictions.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use defuse::{Ctx, Engine, Field, Locality, Operation, Report, Stop, ValueId, Values};

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "f" {
            vec![(v.field("d"), Locality::Local)]
        } else {
            Vec::new()
        })
    }
    fn predicted_defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        (tag(v, a) == "f").then(|| vec![(v.field("d"), v.int(100))])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) == "f" {
            // Reads the previous finisher's d too (a chain).
            let prev = cx.use_field(v.field("d"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
            let base = cx.use_field(v.field("base"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
            let d = v.int(base + prev % 7 + v.as_int(a[1]).unwrap_or(0));
            cx.define(v.field("d"), d);
            return Ok(d);
        }
        Ok(cx.use_field(v.field("d"))?.unwrap_or(ValueId::UNIT))
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
    let pred = || defuse::Greedy::speculative(defuse::Speculation::Predicted, 256);
    match i % 6 {
        0 => eng.evaluate_sequential(),
        1 => eng.evaluate(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
            &mut defuse::Sequential,
        ),
        2 => eng.evaluate(
            &mut defuse::RandomScheduler::new(i as u64, true),
            &mut defuse::Sequential,
        ),
        3 => eng.evaluate(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
            &mut Threads,
        ),
        4 => eng.evaluate(&mut pred(), &mut defuse::Sequential),
        _ => eng.evaluate(&mut pred(), &mut Threads),
    }
    .expect("evaluate")
}

fn build(base: i64) -> (Engine, Vec<defuse::NodeId>, defuse::UnitId) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let (b, bv) = (eng.values().field("base"), eng.values().int(base));
    eng.set_env(u, b, bv);
    let mut ns = Vec::new();
    for i in 0..12 {
        let (f, k) = (eng.values().str("f"), eng.values().int(i));
        ns.push(eng.push(u, op, vec![f, k]));
        let r = eng.values().str("r");
        ns.push(eng.push(u, op, vec![r]));
    }
    (eng, ns, u)
}

fn outs(eng: &Engine, ns: &[defuse::NodeId]) -> Vec<Option<ValueId>> {
    ns.iter().map(|&n| eng.output(n)).collect()
}

#[test]
fn readers_of_predictions_follow_a_changed_recommit() {
    for i in 0..120 {
        let (mut eng, ns, u) = build(1);
        eval(&mut eng, i);
        assert!(eng.check_reads().is_empty(), "cold {i}: {:?}", eng.check_reads());
        for base in [5, 100, 3] {
            let (b, bv) = (eng.values().field("base"), eng.values().int(base));
            eng.set_env(u, b, bv);
            eval(&mut eng, i + 1);
            assert!(
                eng.check_reads().is_empty(),
                "base {base} sched {i}: {:?}",
                eng.check_reads()
            );
            let (mut cold, cns, _) = build(base);
            eval(&mut cold, 0);
            let a: Vec<_> = outs(&eng, &ns)
                .iter()
                .map(|x| x.and_then(|x| eng.values().as_int(x)))
                .collect();
            let c: Vec<_> = outs(&cold, &cns)
                .iter()
                .map(|x| x.and_then(|x| cold.values().as_int(x)))
                .collect();
            assert_eq!(a, c, "base {base} sched {i}");
        }
    }
}
