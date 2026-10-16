//! A pending claimer that sets its claim and then stops releases the
//! siblings past the claim at once (they do not wait for its commit).

use defuse::{Claim, Ctx, Engine, Field, Locality, OpId, Operation, Stop, ValueId, Values};

/// `["c"]`: a pending claimer; its run claims 0 siblings, then reads the
/// environment field `e` (absent until supplied). `["s", i]`: outputs `i`.
struct Ops;

impl Operation for Ops {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn claim(&self, a: &[ValueId], v: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if v.as_str(a[0]).as_deref() == Some("c") {
            Claim::Pending
        } else {
            Claim::None
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if v.as_str(a[0]).as_deref() == Some("c") {
            cx.claim(0);
            let e = cx.use_env(v.field("e"))?;
            return Ok(e.unwrap_or(ValueId::UNIT));
        }
        Ok(a[1])
    }
}

#[test]
fn siblings_run_while_the_claimer_waits() {
    for threads in [false, true] {
        let mut eng = Engine::new();
        let op = eng.register(std::sync::Arc::new(Ops));
        let u = eng.new_unit();
        let c = {
            let x = eng.values().str("c");
            eng.push(u, op, vec![x])
        };
        let sibs: Vec<_> = (0..5)
            .map(|i| {
                let (s, n) = (eng.values().str("s"), eng.values().int(i));
                eng.push(u, op, vec![s, n])
            })
            .collect();
        let rep = if threads {
            eng.evaluate(&mut defuse::Greedy::default(), &mut defuse::Sequential)
        } else {
            eng.evaluate_sequential()
        }
        .unwrap();
        for (i, &s) in sibs.iter().enumerate() {
            assert_eq!(eng.output(s), Some(eng.values().int(i as i64)), "sibling {i} ran");
        }
        assert_eq!(eng.output(c), None, "the claimer still waits on the environment");
        assert!(
            rep.waits_by_kind.iter().all(|(k, _)| *k != "claim"),
            "{:?}",
            rep.waits_by_kind
        );
        // Supplied, the claimer finishes; the siblings stand.
        let (f, x) = (eng.values().field("e"), eng.values().int(9));
        eng.set_environment([(f, x)]);
        eng.evaluate_sequential().unwrap();
        assert_eq!(eng.output(c), Some(eng.values().int(9)));
        assert_eq!(eng.output(sibs[4]), Some(eng.values().int(4)));
    }
}
