//! `Engine::set_environment_base`: a lazy layer under the supplied
//! environment. A read no definition reaches (`use_field`) or of the
//! environment (`use_env`) finds a supplied entry, else an absent one,
//! else the base's answer (asked once per field for the engine's life),
//! else (`use_env` only) the provider's. Supplying a field over the base re-asks its
//! readers; taking it away gives them the base's again.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Stop, ValueId, Values};

struct Ops;

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let f = v.field(&v.as_str(a[1]).unwrap());
        let x = if v.as_str(a[0]).as_deref() == Some("env") { cx.use_env(f)? } else { cx.use_field(f)? };
        Ok(x.unwrap_or_else(|| v.int(-1)))
    }
}

#[test]
fn reads_find_the_base_under_the_supplied_environment() {
    static ASKED: AtomicU64 = AtomicU64::new(0);
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    // The base knows `b<i>` (= i); the provider knows `p` (= 99).
    eng.set_environment_base(Arc::new(|v: &Values, f: Field| {
        ASKED.fetch_add(1, Relaxed);
        v.as_str(f.0).and_then(|n| n.strip_prefix('b').and_then(|i| i.parse::<i64>().ok())).map(|i| v.int(i))
    }));
    eng.set_env_provider(Box::new(|v: &Values, f: Field| (v.as_str(f.0).as_deref() == Some("p")).then(|| Some(v.int(99)))));
    let u = eng.new_unit();
    let mut ids: Vec<NodeId> = Vec::new();
    for (how, name) in [("use", "b1"), ("env", "b2"), ("use", "b1"), ("env", "p"), ("use", "q"), ("env", "b3")] {
        let a = vec![eng.values().str(how), eng.values().str(name)];
        ids.push(eng.push(u, op, a));
    }
    let outs = |eng: &Engine| ids.iter().map(|&x| eng.output(x).and_then(|y| eng.values().as_int(y))).collect::<Vec<_>>();
    let run = |eng: &mut Engine| {
        eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 64), &mut defuse::Sequential).expect("evaluate");
        assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    };
    run(&mut eng);
    assert_eq!(outs(&eng), vec![Some(1), Some(2), Some(1), Some(99), Some(-1), Some(3)]);
    // (b1, b2, b3, and p and q, which the base does not know: once each.)
    let asked = ASKED.load(Relaxed);
    assert!(asked <= 5, "base asked {asked} times");
    // Supplied over the base, and declared absent over it.
    let (b1, b3, seven) = (eng.values().field("b1"), eng.values().field("b3"), eng.values().int(7));
    eng.set_environment([(b1, seven)]);
    eng.set_environment_absent([b3]);
    run(&mut eng);
    assert_eq!(outs(&eng), vec![Some(7), Some(2), Some(7), Some(99), Some(-1), Some(-1)]);
    // Taken away: the base's again, never asked again.
    eng.set_environment([]);
    eng.set_environment_absent([]);
    run(&mut eng);
    assert_eq!(outs(&eng), vec![Some(1), Some(2), Some(1), Some(99), Some(-1), Some(3)]);
    assert_eq!(ASKED.load(Relaxed), asked, "the base asked again");
    // (`environment` leaves the base out.)
    assert!(eng.environment().all(|(f, _)| eng.values().as_str(f.0).as_deref() == Some("p")));
}
