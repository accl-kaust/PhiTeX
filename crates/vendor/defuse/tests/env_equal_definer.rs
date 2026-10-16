//! A reader of the environment's `g = 7` stands when a definer of `g = 7`
//! appears before it (the value it read is the same), and follows that
//! definer: a later change of its value reaches the reader.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops {
    reads: AtomicU64,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "d" {
            vec![(v.field("g"), Locality::Local)]
        } else {
            Vec::new()
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) == "d" {
            cx.define(v.field("g"), a[1]);
            return Ok(ValueId::UNIT);
        }
        self.reads.fetch_add(1, Relaxed);
        Ok(cx.use_field(v.field("g"))?.unwrap_or(ValueId::UNIT))
    }
}

#[test]
fn an_equal_new_definer_keeps_an_environment_reader() {
    let ops = Arc::new(Ops {
        reads: AtomicU64::new(0),
    });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    let u = eng.new_unit();
    let (g, seven) = (eng.values().field("g"), eng.values().int(7));
    eng.set_env(u, g, seven);
    let r = eng.values().str("r");
    let rn = eng.push(u, op, vec![r]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(eng.output(rn), Some(seven));
    let before = ops.reads.load(Relaxed);
    let d = eng.values().str("d");
    let dn = eng.insert_before(rn, op, vec![d, seven]);
    let rep = eng.evaluate_sequential().unwrap();
    assert_eq!(ops.reads.load(Relaxed), before, "R stands: {:?}", rep.rerun_causes);
    let eight = eng.values().int(8);
    eng.replace(dn, op, vec![d, eight]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(eng.output(rn), Some(eight), "a change at the new definer reaches R");
}
