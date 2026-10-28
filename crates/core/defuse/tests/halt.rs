//! `Ctx::halt`: a run asks to end the evaluation; the request is granted
//! at the quiet point if the run still stands (TeX's fatal error ends the
//! job), `Report::halted` says so, and a later evaluation goes on.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, Operation, Stop, ValueId, Values};

/// `[name, x, halt]`: defines `name` := x; with `halt` != 0 it asks to
/// end the evaluation. Counts its runs.
struct Def {
    runs: AtomicU64,
}

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let a = cx.args().to_vec();
        cx.define(Field(a[0]), a[1]);
        if v.as_int(a[2]).unwrap_or(0) != 0 {
            cx.halt();
        }
        Ok(a[1])
    }
}

/// `[from]`: reads `from` as the unit's last definition and asks to halt
/// if its value is 1. Counts its runs.
struct HaltIf {
    runs: AtomicU64,
}

impl Operation for HaltIf {
    fn name(&self) -> &'static str {
        "haltif"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let x = match cx.read_at(Field(cx.args()[0]), phitex_defuse::At::End(cx.unit()))? {
            phitex_defuse::FieldAt::Def(x, _) => Some(x),
            _ => None,
        };
        if x.and_then(|x| v.as_int(x)) == Some(1) {
            cx.halt();
        }
        Ok(x.unwrap_or(ValueId::UNIT))
    }
}

/// A run asks to halt; the evaluation ends at its quiet point with
/// `halted`; the halting node committed; a later evaluation, the halt
/// gone, is not halted and everything stands.
#[test]
fn a_halt_is_granted_at_the_quiet_point_and_the_next_evaluation_goes_on() {
    let e = Engine::new();
    let op = Arc::new(Def { runs: AtomicU64::new(0) });
    let def = e.register(op.clone());
    let v = e.values();
    let u = e.new_unit();
    let a = e.push(u, def, vec![v.field("a").0, v.int(1), v.int(1)]);
    let rest: Vec<_> = (0..50).map(|i| e.push(u, def, vec![v.field(&format!("b{i}")).0, v.int(i), v.int(0)])).collect();
    let rep = e.evaluate(4).unwrap();
    assert!(rep.halted, "{rep:?}");
    assert_eq!(e.output(a).and_then(|x| v.as_int(x)), Some(1), "the halting run committed: {rep:?}");
    e.replace(a, def, vec![v.field("a").0, v.int(2), v.int(0)]);
    let rep = e.evaluate(4).unwrap();
    assert!(!rep.halted, "{rep:?}");
    assert_eq!(e.output(a).and_then(|x| v.as_int(x)), Some(2));
    for (i, &n) in rest.iter().enumerate() {
        assert_eq!(e.output(n).and_then(|x| v.as_int(x)), Some(i as i64), "node {i}: {rep:?}");
    }
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// A halt asked on a guess that is refuted is not granted: H reads g as
/// the unit's last definition and halts on 1; P after it is predicted to
/// define g := 1 but defines 2. H runs first on one thread (the
/// prediction), asks to halt, P runs, H is re-asked and runs on 2 without
/// asking: the evaluation ends not halted, with every node run.
#[test]
fn a_halt_asked_on_a_refuted_guess_is_not_granted() {
    let e = Engine::new();
    let def = Arc::new(Def { runs: AtomicU64::new(0) });
    let def_op = e.register(def.clone());
    let pred = e.register(Arc::new(PredTwo));
    let h = Arc::new(HaltIf { runs: AtomicU64::new(0) });
    let haltif = e.register(h.clone());
    let v = e.values();
    let u = e.new_unit();
    let g = v.field("g");
    let hn = e.push(u, haltif, vec![g.0]);
    let _p = e.push(u, pred, vec![g.0, v.int(1)]);
    let rest: Vec<_> = (0..20).map(|i| e.push(u, def_op, vec![v.field(&format!("b{i}")).0, v.int(i), v.int(0)])).collect();
    let rep = e.evaluate(1).unwrap();
    assert!(!rep.halted, "the guess was refuted, the halt not granted: {rep:?}");
    assert_eq!(e.output(hn).and_then(|x| v.as_int(x)), Some(2), "{rep:?}");
    assert_eq!(h.runs.load(Ordering::SeqCst), 2, "H ran on the guess and on the truth: {rep:?}");
    assert!(rest.iter().all(|&n| e.output(n).is_some()));
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// `[name, x]`: predicted to define `name` := x, defines x + 1.
struct PredTwo;

impl Operation for PredTwo {
    fn name(&self) -> &'static str {
        "predtwo"
    }

    fn predicted_defs(&self, args: &[ValueId], _: &Values) -> Option<Vec<(Field, ValueId)>> {
        Some(vec![(Field(args[0]), args[1])])
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args().to_vec();
        let x = v.int(v.as_int(a[1]).unwrap_or(0) + 1);
        cx.define(Field(a[0]), x);
        Ok(x)
    }
}
