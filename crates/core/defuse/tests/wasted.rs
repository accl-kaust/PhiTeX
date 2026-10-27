//! The ledger's wasted runs (`Report::runs_wasted`): a run after a run of
//! the same node that left every output as it was (its definitions,
//! contributions, output, brackets and placements) is wasted; one whose
//! definition changed is not.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Operation, Stop, ValueId};

/// `[name, x]`: defines `name` := x.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[from, to, c]`: reads field `from` and defines `to` := c (a constant,
/// whatever it read: a rerun of it is wasted), or, with `c` unit, the
/// value read (a rerun after a change of `from` is not).
struct Copy;

impl Operation for Copy {
    fn name(&self) -> &'static str {
        "copy"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT);
        let c = cx.args()[2];
        let y = if c == ValueId::UNIT { x } else { c };
        cx.define(Field(cx.args()[1]), y);
        Ok(y)
    }
}

#[test]
fn a_rerun_with_identical_outputs_counts_as_wasted() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let copy = e.register(Arc::new(Copy));
    let u = e.new_unit();
    let x = e.push(u, def, vec![v.field("x").0, v.int(1)]);
    let c = e.push(u, copy, vec![v.field("x").0, v.field("y").0, v.int(5)]);
    let rep = e.evaluate(2).unwrap();
    assert_eq!(rep.runs_wasted, 0, "first runs are never wasted: {rep:?}");
    assert_eq!(e.output(c).and_then(|y| v.as_int(y)), Some(5));
    // x changes: the copy is re-asked, runs again, and defines y := 5 as before
    e.replace(x, def, vec![v.field("x").0, v.int(2)]);
    let rep = e.evaluate(2).unwrap();
    assert_eq!(rep.runs_wasted, 1, "the copy's rerun left every output as it was: {rep:?}");
    assert_eq!(rep.wasted_by_op, vec![("copy", 1)], "{rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

#[test]
fn a_rerun_with_a_changed_definition_is_not_wasted() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let copy = e.register(Arc::new(Copy));
    let u = e.new_unit();
    let x = e.push(u, def, vec![v.field("x").0, v.int(1)]);
    let c = e.push(u, copy, vec![v.field("x").0, v.field("y").0, ValueId::UNIT]);
    let rep = e.evaluate(2).unwrap();
    assert_eq!(rep.runs_wasted, 0, "{rep:?}");
    assert_eq!(e.output(c).and_then(|y| v.as_int(y)), Some(1));
    e.replace(x, def, vec![v.field("x").0, v.int(2)]);
    let rep = e.evaluate(2).unwrap();
    assert_eq!(e.output(c).and_then(|y| v.as_int(y)), Some(2), "{rep:?}");
    assert_eq!(rep.runs_wasted, 0, "y changed: the rerun was needed: {rep:?}");
    assert!(rep.wasted_by_op.is_empty(), "{rep:?}");
    // (the replaced definer itself ran once in this evaluation, as a new
    // run of its node, with another definition: not wasted either)
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}
