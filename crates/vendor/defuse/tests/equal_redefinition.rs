//! A definition re-made at another node with an equal value leaves its
//! readers standing: `P [a]` follows `Q [a]`; `Q` defines `g = 7`; `R`
//! reads `g`. Editing `P` to `[b]` re-makes `Q [b]`, which defines `g = 7`.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use defuse::{Ctx, Engine, Field, Locality, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    me: Mutex<Option<OpId>>,
    reads: AtomicU64,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "q" { vec![(v.field("g"), Locality::Local)] } else { Vec::new() })
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
    let ops = Arc::new(Ops { me: Mutex::new(None), reads: AtomicU64::new(0) });
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
    assert_eq!(eng.output(rn), Some(eng.values().int(8)), "a change at the new site reaches R");
}
