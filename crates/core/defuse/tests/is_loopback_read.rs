//! `Ctx::is_loopback` is a read: a run that saw "not a loop-back field" runs
//! again when the field is declared one, also when it stopped and parked
//! after asking.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops;

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let w = v.field("w:k");
        let e = v.field("env:e");
        if cx.parked().is_none() {
            let lb = cx.is_loopback(w);
            let x = v.int(i64::from(lb));
            if cx.args()[0] == v.str("park")
                && let Err(s) = cx.use_env(e)
            {
                cx.park(Box::new(x));
                return Err(s);
            }
            return Ok(x);
        }
        let x = *cx.parked().unwrap().downcast::<ValueId>().unwrap();
        cx.use_env(e)?;
        Ok(x)
    }
}

#[test]
fn declaring_a_loopback_field_reruns_runs_that_asked() {
    for park in [false, true] {
        let mut eng = Engine::new();
        let op = eng.register(Arc::new(Ops));
        let u = eng.new_unit();
        let a = eng.values().str(if park { "park" } else { "plain" });
        let n = eng.push(u, op, vec![a]);
        eng.evaluate_sequential().expect("evaluate");
        let (w, z, e) = (
            eng.values().field("w:k"),
            eng.values().int(0),
            eng.values().field("env:e"),
        );
        eng.declare_loopback(w, z);
        let one = eng.values().int(1);
        eng.set_environment([(e, one)]);
        eng.evaluate_sequential().expect("evaluate");
        assert_eq!(
            eng.output(n).and_then(|x| eng.values().as_int(x)),
            Some(1),
            "park {park}"
        );
        assert!(eng.check_reads().is_empty());
    }
}
