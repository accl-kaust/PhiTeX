//! A loop-back field a run discovers: `B` reads `w:k` (absent), `A` after
//! it declares `w:k` a loop-back field from its run and publishes 5. In one
//! evaluation B ends reading 5, as when `w:k` is declared beforehand.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Report, Stop, ValueId, Values};

struct Ops;

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
        let w = v.field("w:k");
        if tag(v, &a) == "a" {
            cx.declare_loopback(w, v.int(0));
            cx.publish(w, v.int(5));
            return Ok(ValueId::UNIT);
        }
        Ok(cx.use_field(w)?.unwrap_or(v.int(-1)))
    }
}

fn build(pre: bool) -> (Engine, phitex_defuse::NodeId) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    if pre {
        let (w, z) = (eng.values().field("w:k"), eng.values().int(0));
        eng.declare_loopback(w, z);
    }
    let b = eng.values().str("b");
    let bn = eng.push(u, op, vec![b]);
    let a = eng.values().str("a");
    eng.push(u, op, vec![a]);
    (eng, bn)
}

fn eval(eng: &mut Engine, i: usize) -> Report {
    match i % 3 {
        0 => eng.evaluate_sequential(),
        1 => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 256),
            &mut phitex_defuse::Sequential,
        ),
        _ => eng.evaluate(
            &mut phitex_defuse::RandomScheduler::new(i as u64, true),
            &mut phitex_defuse::Sequential,
        ),
    }
    .expect("evaluate")
}

#[test]
fn a_loopback_field_declared_by_a_run_works_in_the_same_evaluation() {
    for i in 0..30 {
        let (mut want, wb) = build(true);
        eval(&mut want, i);
        let (mut eng, b) = build(false);
        let rep = eval(&mut eng, i);
        let int = |e: &Engine, n| e.output(n).and_then(|x| e.values().as_int(x));
        assert_eq!(int(&eng, b), int(&want, wb), "schedule {i}: {rep:?}");
        assert_eq!(int(&eng, b), Some(5));
    }
}
