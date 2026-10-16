//! 15 000 nodes, each with its claim and scope effect pending (known when
//! it runs), reading `x` and claiming nothing; the first, `["i"]`, has a
//! pending definition set and waits for the environment field `file` (so
//! every other node waits on it). Rounds that supply unrelated fields
//! (nothing to run) cost what they wake, not what is stalled.

use std::sync::Arc;

use defuse::{Claim, Ctx, Effect, Engine, Field, Locality, OpId, Operation, Stop, ValueId, Values};

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        if tag(v, a) == "i" { None } else { Some(Vec::new()) }
    }
    fn claim(&self, _a: &[ValueId], _v: &Values, _f: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        Claim::Pending
    }
    fn effect(&self, _a: &[ValueId], _v: &Values) -> Effect {
        Effect::Pending
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.claim(0);
        cx.set_effect(Effect::None);
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) == "i" {
            if let Some(x) = cx.use_env(v.field("file"))? {
                cx.define(v.field("x"), x);
            }
            return Ok(ValueId::UNIT);
        }
        Ok(cx.use_field(v.field("x"))?.unwrap_or_else(|| v.str("none")))
    }
}

#[test]
fn rounds_that_wake_nothing_are_cheap() {
    let n: usize = std::env::var("N").ok().and_then(|x| x.parse().ok()).unwrap_or(15_000);
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let i = eng.values().str("i");
    eng.push(u, op, vec![i]);
    let mut last = None;
    for _ in 0..n {
        let r = eng.values().str("r");
        last = Some(eng.push(u, op, vec![r]));
    }
    eng.add_root(last.unwrap());
    let t0 = std::time::Instant::now();
    eng.evaluate_sequential().expect("evaluate");
    let first = t0.elapsed();
    let mut times = Vec::new();
    for k in 0..3 {
        let (g, one) = (eng.values().field(&format!("other{k}")), eng.values().int(1));
        eng.set_environment(vec![(g, one)]);
        let t = std::time::Instant::now();
        eng.evaluate_sequential().expect("evaluate");
        times.push(t.elapsed());
    }
    eprintln!("n {n}: first {first:?}, then {times:?}");
    assert!(times.iter().all(|t| t.as_secs_f64() < 1.0), "rounds with nothing to run took {times:?} (first {first:?})");
}
