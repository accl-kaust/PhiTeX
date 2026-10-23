//! A careful run guesses again once past its start: a refuted node's run
//! after it (careful, on committed values only) lapses at its second mark,
//! though it did not resume at a mark. Unit [d, r]: `d` defines `x`
//! (predicted 5, made 7); `r` marks, reads `x`, marks again and outputs
//! `x`. In one step `r` runs ahead of `d` on the prediction and is
//! refuted; its next run is careful and lapses at its second mark.

use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "d" { vec![(v.field("x"), Locality::Global)] } else { Vec::new() })
    }
    fn predicted_defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        (tag(v, a) == "d").then(|| vec![(v.field("x"), v.int(5))])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) == "d" {
            cx.define_with(v.field("x"), v.int(7), Locality::Global);
            return Ok(ValueId::UNIT);
        }
        cx.checkpoint(0, v.int(0))?;
        let x = cx.use_field(v.field("x"))?.unwrap_or(ValueId::UNIT);
        cx.checkpoint(1, x)?;
        Ok(x)
    }
}

#[test]
fn a_careful_run_lapses_past_its_first_mark() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let d_args = vec![eng.values().str("d")];
    eng.push(u, op, d_args);
    let r_args = vec![eng.values().str("r")];
    let r = eng.push(u, op, r_args);
    let rep = eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 64), &mut defuse::Sequential).expect("evaluate");
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    assert_eq!(eng.output(r).and_then(|x| eng.values().as_int(x)), Some(7));
    assert!(rep.mispredictions >= 1, "{rep:?}");
    assert!(rep.careful_lapsed >= 1, "{rep:?}");
}
