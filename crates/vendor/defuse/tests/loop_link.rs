//! A read of a loop-back field's round value is linked in the chain to
//! the commit that ended the round before (`Cause::Loop`): a rerun for the
//! next round waited for it. Unit [p, r]: `p` reads the loop-back field
//! `n` and publishes `n + 1` up to 2; `r` reads `n`.

use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops;

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let n = v.field("n");
        let x = cx.use_field(n)?.and_then(|y| v.as_int(y)).unwrap_or(0);
        if v.as_str(a[0]).as_deref() == Some("p") {
            cx.publish(n, v.int((x + 1).min(2)));
        }
        Ok(v.int(x))
    }
}

#[test]
fn a_round_value_links_to_the_round_end() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    eng.set_heights(true);
    let (n, zero) = (eng.values().field("n"), eng.values().int(0));
    eng.declare_loopback(n, zero);
    let u = eng.new_unit();
    let p = vec![eng.values().str("p")];
    eng.push(u, op, p);
    let r = vec![eng.values().str("r")];
    let r = eng.push(u, op, r);
    let rep = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(eng.output(r).and_then(|y| eng.values().as_int(y)), Some(2));
    assert!(rep.rounds >= 3, "rounds {}", rep.rounds);
    let chain = eng.critical_chain_by_time();
    assert!(chain.iter().any(|(_, why, _)| matches!(why, defuse::Cause::Loop { field } if *field == n)), "{chain:?}");
}
