//! The run audit (`Engine::set_run_audit`): a rerun whose effects all come
//! out the same is reported wasted, with what made it run; one whose
//! output changed is not. Unit [d, r, s]: `d` defines `x` (`a` then
//! `b`); `r` reads `x` and outputs its length class (the same for both);
//! `s` reads `x` and outputs it.

use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, Operation, Rerun, Stop, ValueId, Values};

struct Ops;

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if v.as_str(a[0]).as_deref() == Some("d") { vec![(v.field("x"), Locality::Local)] } else { Vec::new() })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let x = v.field("x");
        match v.as_str(a[0]).as_deref() {
            Some("d") => {
                cx.define(x, a[1]);
                Ok(ValueId::UNIT)
            }
            Some("r") => {
                let y = cx.use_field(x)?;
                Ok(v.int(y.and_then(|y| v.as_str(y)).map_or(0, |s| s.len() as i64)))
            }
            _ => Ok(cx.use_field(x)?.unwrap_or(ValueId::UNIT)),
        }
    }
}

#[test]
fn a_rerun_that_changes_nothing_is_reported_wasted() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    eng.set_run_audit(true);
    let u = eng.new_unit();
    let d = vec![eng.values().str("d"), eng.values().str("a")];
    let d = eng.push(u, op, d);
    let r = vec![eng.values().str("r")];
    let r = eng.push(u, op, r);
    let s = vec![eng.values().str("s")];
    let s = eng.push(u, op, s);
    let rep = eng.evaluate_sequential().unwrap();
    // (Cold: no reruns.)
    let a = rep.audit.expect("audited");
    assert_eq!((a.reruns, a.wasted.len()), (0, 0));
    let b = vec![eng.values().str("d"), eng.values().str("b")];
    eng.replace(d, op, b);
    let rep = eng.evaluate_sequential().unwrap();
    let a = rep.audit.expect("audited");
    let x = eng.values().field("x");
    // `d` (new output? no: unit) changed its definition; `s` its output;
    // `r` nothing.
    assert_eq!(a.reruns, 3, "{a:?}");
    assert_eq!(a.wasted.len(), 1, "{a:?}");
    assert_eq!(a.wasted[0].node, r);
    assert!(matches!(a.wasted[0].cause, Some(Rerun::Changed { field, .. }) if field == x), "{a:?}");
    assert!(!a.wasted.iter().any(|w| w.node == s));
    assert_eq!(a.by_cause().len(), 1);
    // Off: none.
    eng.set_run_audit(false);
    let c = vec![eng.values().str("d"), eng.values().str("c")];
    eng.replace(d, op, c);
    assert!(eng.evaluate_sequential().unwrap().audit.is_none());
}
