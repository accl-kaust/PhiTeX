//! `Engine::set_heights(false)` skips the height pass: the report's
//! critical path is 0 and the chain empty, outputs unchanged; turned on
//! again, the next evaluation computes them all.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops;

impl Operation for Ops {
    fn defs(&self, _: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(vec![(v.field("d"), Locality::Local)])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let prev = cx.use_field(v.field("d"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
        let d = v.int(prev + v.as_int(a[0]).unwrap_or(0));
        cx.define(v.field("d"), d);
        Ok(d)
    }
}

#[test]
fn heights_can_be_turned_off_and_on() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let ns: Vec<_> = (0..20)
        .map(|i| {
            let x = eng.values().int(i);
            eng.push(u, op, vec![x])
        })
        .collect();
    eng.set_heights(false);
    let rep = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(rep.critical_path, 0);
    assert!(eng.critical_chain().is_empty());
    let last = eng.output(ns[19]).and_then(|x| eng.values().as_int(x));
    assert_eq!(last, Some((0..20).sum()));
    eng.set_heights(true);
    let x = eng.values().int(100);
    eng.replace(ns[0], op, vec![x]);
    let rep = eng.evaluate_sequential().expect("evaluate");
    assert!(rep.critical_path >= 19, "critical path {}", rep.critical_path);
    assert!(!eng.critical_chain().is_empty());
}
