//! A refutation that is a round's news does not make a node careful:
//! unit [a, f_0 .. f_N, p]; `a` reads loop-back field `last` and defines
//! `al` from it, predicting `al = 0`; each `f_i` reads `al` (and the
//! previous finisher's `d`, predicted by its definer) and defines `d`;
//! `p` publishes `last = N`. Round 1 reads `last = 0`: the finishers'
//! guess of `al` holds. Round 2 reads `last = N`: `a` defines `al = N`
//! and every finisher that guessed `al = 0` is refuted, by the loop-back
//! value's move, not by a guess that keeps failing: none runs carefully.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

const N: i64 = 24;

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "a" => vec![(v.field("al"), Locality::Global)],
            "f" => vec![(v.field("d"), Locality::Global)],
            _ => Vec::new(),
        })
    }
    fn predicted_defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        match tag(v, a).as_str() {
            "a" => Some(vec![(v.field("al"), v.int(0))]),
            "f" => Some(vec![(v.field("d"), a[1])]),
            _ => None,
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "a" => {
                let last = cx.use_field(v.field("last"))?.unwrap_or(v.int(0));
                cx.define_with(v.field("al"), last, Locality::Global);
                Ok(last)
            }
            "f" => {
                let al = cx.use_field(v.field("al"))?.and_then(|x| v.as_int(x)).unwrap_or(-1);
                let _ = cx.use_field(v.field("d"))?;
                cx.define_with(v.field("d"), a[1], Locality::Global);
                Ok(v.int(al))
            }
            "p" => {
                cx.publish(v.field("last"), v.int(N));
                Ok(ValueId::UNIT)
            }
            _ => Ok(ValueId::UNIT),
        }
    }
}

#[test]
fn a_refutation_by_a_loop_back_move_is_not_careful() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let last = eng.values().field("last");
    let zero = eng.values().int(0);
    eng.declare_loopback(last, zero);
    let u = eng.new_unit();
    let a = vec![eng.values().str("a")];
    eng.push(u, op, a);
    let fs: Vec<_> = (0..N)
        .map(|i| {
            let args = vec![eng.values().str("f"), eng.values().int(i)];
            eng.push(u, op, args)
        })
        .collect();
    let p = vec![eng.values().str("p")];
    eng.push(u, op, p);
    let rep = eng
        .evaluate(&mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::Predicted, 256), &mut phitex_defuse::Sequential)
        .expect("evaluate");
    for &f in &fs {
        assert_eq!(eng.output(f).and_then(|x| eng.values().as_int(x)), Some(N));
    }
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    assert!(rep.mispredictions > 0, "the finishers guessed `al` and were refuted: {rep:?}");
    assert_eq!(rep.nodes_conservative, 0, "refuted by the round's news only: {rep:?}");
}
