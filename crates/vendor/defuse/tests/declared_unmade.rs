//! A global entry and a local one of a field share a key, not an entry:
//! a node whose last run defined `l` one way, its text now declaring it the
//! other, leaves no entry of either behind when its run defines nothing, or
//! when it goes before running.

use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops;

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match v.as_str(a[0]).as_deref() {
            Some("g") => vec![(v.field("l"), Locality::Global)],
            Some("a" | "loc") => vec![(v.field("l"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let l = v.field("l");
        Ok(match v.as_str(a[0]).as_deref() {
            Some("g") => {
                cx.define_with(l, v.int(2), Locality::Global);
                ValueId::UNIT
            }
            Some("loc") => {
                cx.define_with(l, v.int(3), Locality::Local);
                ValueId::UNIT
            }
            Some("r") => cx.use_field(l)?.unwrap_or_else(|| v.str("none")),
            _ => ValueId::UNIT,
        })
    }
}

#[test]
fn a_global_definition_retexted_to_an_unmade_local_one_goes() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let (g, rd) = (eng.values().str("g"), eng.values().str("r"));
    let d = eng.push(u, op, vec![g]);
    let r = eng.push(u, op, vec![rd]);
    eng.evaluate_sequential().expect("evaluate");
    assert_eq!(eng.values().render(eng.output(r).unwrap()), "2");
    let a = vec![eng.values().str("a")];
    eng.replace(d, op, a);
    eng.evaluate_sequential().expect("evaluate");
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    assert_eq!(eng.values().render(eng.output(r).unwrap()), "\"none\"");
}

#[test]
fn a_local_definition_retexted_to_a_global_one_and_gone_leaves_nothing() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let (lc, rd) = (eng.values().str("loc"), eng.values().str("r"));
    let d = eng.push(u, op, vec![lc]);
    let r = eng.push(u, op, vec![rd]);
    eng.evaluate_sequential().expect("evaluate");
    assert_eq!(eng.values().render(eng.output(r).unwrap()), "3");
    // Retexted (its global entry filed pending), then gone, unevaluated.
    let g = vec![eng.values().str("g")];
    eng.replace(d, op, g);
    eng.remove(d);
    eng.evaluate_sequential().expect("evaluate");
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    assert_eq!(eng.values().render(eng.output(r).unwrap()), "\"none\"");
}
