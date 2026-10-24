//! A rerun for an environment change names the field the reader read, not
//! another of the fields that changed with it (`Rerun::Environment`).

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Rerun, Stop, ValueId, Values};

struct Ops;

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let f = v.field(&v.as_str(cx.args()[0]).unwrap());
        Ok(cx.use_field(f)?.unwrap_or(ValueId::UNIT))
    }
}

#[test]
fn an_environment_rerun_names_the_field_read() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let fields: Vec<Field> = (0..8).map(|i| eng.values().field(&format!("e{i}"))).collect();
    let env = |eng: &Engine, k: i64| -> Vec<(Field, ValueId)> { fields.iter().map(|&f| (f, eng.values().int(k))).collect() };
    let e0 = env(&eng, 0);
    eng.set_environment(e0);
    let u = eng.new_unit();
    let readers: Vec<NodeId> = (0..8).map(|i| {
        let a = vec![eng.values().str(&format!("e{i}"))];
        eng.push(u, op, a)
    }).collect();
    eng.evaluate_sequential().unwrap();
    let e1 = env(&eng, 1);
    eng.set_environment(e1);
    let rep = eng.evaluate_sequential().unwrap();
    for (i, r) in readers.iter().enumerate() {
        let why = rep.rerun_causes.iter().find(|c| c.0 == *r).map(|c| c.1);
        assert_eq!(why, Some(Rerun::Environment { field: fields[i] }), "reader of e{i}");
    }
}
