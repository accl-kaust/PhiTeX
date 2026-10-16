//! `claim_limit` as a prediction: siblings past it run before the claim is
//! known; a claim that then reaches past the limit takes them back, and
//! readers of what they defined see the claimed result.

use defuse::{Claim, Ctx, Engine, Field, FieldAt, Locality, OpId, Operation, Stop, ValueId, Values};

/// `["c"]` claims `e` siblings (an environment field), limit 1;
/// `["d", k]` defines `x = k`; `["r"]` outputs `x` (-1 when unset).
struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "d" {
            vec![(v.field("x"), Locality::Local)]
        } else {
            Vec::new()
        })
    }
    fn claim(&self, a: &[ValueId], v: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if tag(v, a) == "c" { Claim::Pending } else { Claim::None }
    }
    fn claim_limit(
        &self,
        a: &[ValueId],
        v: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Option<usize> {
        (tag(v, a) == "c").then_some(1)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "c" => {
                let n = cx.use_env(v.field("e"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                cx.claim(usize::try_from(n).unwrap());
                Ok(ValueId::UNIT)
            }
            "d" => {
                cx.define(v.field("x"), a[1]);
                Ok(ValueId::UNIT)
            }
            _ => {
                let r = cx.use_field_at(v.field("x"), None)?;
                let _ = r;
                let x = cx.use_field(v.field("x"))?;
                Ok(x.unwrap_or_else(|| v.int(-1)))
            }
        }
    }
}

fn build(env_first: bool) -> (Engine, defuse::NodeId) {
    let mut eng = Engine::new();
    let op = eng.register(std::sync::Arc::new(Ops));
    let u = eng.new_unit();
    let s = |e: &Engine, t: &str| e.values().str(t);
    let c = s(&eng, "c");
    eng.push(u, op, vec![c]);
    for k in 1..=2 {
        let (d, n) = (s(&eng, "d"), eng.values().int(k));
        eng.push(u, op, vec![d, n]);
    }
    let r = s(&eng, "r");
    let rd = eng.push(u, op, vec![r]);
    if env_first {
        let (f, x) = (eng.values().field("e"), eng.values().int(2));
        eng.set_environment([(f, x)]);
    }
    (eng, rd)
}

#[test]
fn a_claim_past_its_limit_takes_back_what_ran() {
    let _ = FieldAt::Unset;
    // Reference: the claim known from the start.
    let (mut want, rd0) = build(true);
    want.evaluate_sequential().unwrap();
    let expect = want.output(rd0);
    // Predicted: the claimer waits; the sibling past the limit runs first.
    let (mut eng, rd) = build(false);
    eng.evaluate_sequential().unwrap();
    assert_eq!(
        eng.output(rd),
        Some(eng.values().int(2)),
        "before the claim, r reads d 2"
    );
    let (f, x) = (eng.values().field("e"), eng.values().int(2));
    eng.set_environment([(f, x)]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(
        eng.output(rd).map(|v| eng.values().as_int(v)),
        expect.map(|v| want.values().as_int(v)),
        "after the claim, r sees what it would have"
    );
}
