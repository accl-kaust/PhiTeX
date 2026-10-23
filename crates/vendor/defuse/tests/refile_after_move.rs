//! A reader of a node that moved stays filed as its reader: freeing the
//! node re-asks its readers without unfiling them, so one that runs again
//! reading it with the same entries (a diff of them files nothing) is
//! still re-asked when the node's output changes. Unit [d, r]: `d`
//! defines `y`; `r` places children `a` and `x` (other operations) in an
//! order its argument picks and outputs `x`'s output, `y`. Placing `x`
//! first moves it (made again ahead of `a`); `r` runs again on the new
//! `x`. Then `y` changes: `x` runs again, and so must `r`.

use std::sync::{Arc, OnceLock};

use defuse::{Ctx, Engine, Field, Locality, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    x: OnceLock<OpId>,
    a: OnceLock<OpId>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "d" { vec![(v.field("y"), Locality::Global)] } else { Vec::new() })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "d" => {
                cx.define_with(v.field("y"), a[1], Locality::Global);
                Ok(ValueId::UNIT)
            }
            "x" => Ok(cx.use_field(v.field("y"))?.unwrap_or(ValueId::UNIT)),
            "a" => Ok(ValueId::UNIT),
            _ => {
                let (x, an) = (*self.x.get().expect("registered"), *self.a.get().expect("registered"));
                let i = if v.as_int(a[1]) == Some(1) {
                    cx.child(an, vec![v.str("a")]);
                    cx.child(x, vec![v.str("x")])
                } else {
                    let i = cx.child(x, vec![v.str("x")]);
                    cx.child(an, vec![v.str("a")]);
                    i
                };
                cx.output_of(i)
            }
        }
    }
}

#[test]
fn a_reader_of_a_moved_node_is_re_asked_when_its_output_changes() {
    let mut eng = Engine::new();
    let ops = Arc::new(Ops { x: OnceLock::new(), a: OnceLock::new() });
    let r_op = eng.register(ops.clone());
    let _ = ops.x.set(eng.register(ops.clone()));
    let _ = ops.a.set(eng.register(ops.clone()));
    let u = eng.new_unit();
    let a = vec![eng.values().str("d"), eng.values().int(1)];
    let d = eng.push(u, r_op, a);
    let a = vec![eng.values().str("r"), eng.values().int(1)];
    let r = eng.push(u, r_op, a);
    eng.add_root(r);
    let out = |eng: &Engine| eng.output(r).and_then(|y| eng.values().as_int(y));
    eng.evaluate_sequential().expect("evaluate");
    assert_eq!(out(&eng), Some(1));
    // `x` first: moved ahead of `a`.
    let a = vec![eng.values().str("r"), eng.values().int(0)];
    eng.replace(r, r_op, a);
    eng.evaluate_sequential().expect("evaluate");
    assert_eq!(out(&eng), Some(1), "x moved");
    // `y` changes: `x`'s output, then `r`'s.
    let a = vec![eng.values().str("d"), eng.values().int(2)];
    eng.replace(d, r_op, a);
    eng.evaluate_sequential().expect("evaluate");
    assert_eq!(out(&eng), Some(2), "y edited");
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
}
