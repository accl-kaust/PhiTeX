//! A loop-back declared after a run read the field while nothing defined
//! it: the reader runs again with the round value.

use defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Reader;

impl Operation for Reader {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        Ok(cx.use_field(v.field("f"))?.unwrap_or(ValueId::UNIT))
    }
}

#[test]
fn declaring_a_loopback_reruns_a_reader_of_the_undefined_field() {
    let mut eng = Engine::new();
    let op = eng.register(std::sync::Arc::new(Reader));
    let u = eng.new_unit();
    let n = eng.push(u, op, vec![]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(eng.output(n), Some(ValueId::UNIT));
    let (f, one) = (eng.values().field("f"), eng.values().int(1));
    eng.declare_loopback(f, one);
    let rep = eng.evaluate_sequential().unwrap();
    assert_eq!(eng.output(n), Some(one), "the reader sees the round value");
    assert_eq!(rep.nodes_run, 1);
}
