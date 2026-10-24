//! A careful run guesses again once past its start: a node refuted twice
//! runs carefully (on committed values only) and lapses at its second
//! mark, though it did not resume at a mark. Unit [c, d, e, f, r]: `c`
//! defines `w` (predicted 5, made 7); `d`, `e` and `f` define `x` as `w`,
//! `y` as `x` and `z` as `y` (each predicted 5); `r` marks, reads `x`,
//! `y` and `z`, marks again and outputs their sum. In one step everything
//! runs ahead on the predictions: `d` is refuted, `r` stands on `d` in
//! work. `d`'s rerun makes `e` and `r` run again; `r` runs beside `e` on
//! `e`'s last result and is refuted by `e`'s commit, then beside `f` on
//! `f`'s, refuted again: its next run is careful and lapses at its second
//! mark.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

/// What each tag defines, and the field it reads for it (`c` none).
fn def_of(t: &str) -> Option<(&'static str, Option<&'static str>)> {
    match t {
        "c" => Some(("w", None)),
        "d" => Some(("x", Some("w"))),
        "e" => Some(("y", Some("x"))),
        "f" => Some(("z", Some("y"))),
        _ => None,
    }
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(def_of(&tag(v, a)).map_or_else(Vec::new, |(f, _)| vec![(v.field(f), Locality::Global)]))
    }
    fn predicted_defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        def_of(&tag(v, a)).map(|(f, _)| vec![(v.field(f), v.int(5))])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let args = cx.args();
        if let Some((f, from)) = def_of(&tag(v, &args)) {
            let val = match from {
                Some(g) => cx.use_field(v.field(g))?.unwrap_or(ValueId::UNIT),
                None => v.int(7),
            };
            cx.define_with(v.field(f), val, Locality::Global);
            return Ok(ValueId::UNIT);
        }
        cx.checkpoint(0, v.int(0))?;
        let read = |cx: &mut Ctx<'_>, f: &str| -> Result<i64, Stop> { Ok(cx.use_field(v.field(f))?.and_then(|n| v.as_int(n)).unwrap_or(0)) };
        let sum = v.int(read(cx, "x")? + read(cx, "y")? + read(cx, "z")?);
        cx.checkpoint(1, sum)?;
        Ok(sum)
    }
}

#[test]
fn a_careful_run_lapses_past_its_first_mark() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    for t in ["c", "d", "e", "f"] {
        let args = vec![eng.values().str(t)];
        eng.push(u, op, args);
    }
    let r_args = vec![eng.values().str("r")];
    let r = eng.push(u, op, r_args);
    let rep = eng.evaluate(&mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::Predicted, 64), &mut phitex_defuse::Sequential).expect("evaluate");
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    assert_eq!(eng.output(r).and_then(|x| eng.values().as_int(x)), Some(21));
    assert!(rep.mispredictions >= 3, "{rep:?}");
    assert!(rep.careful_lapsed >= 1, "{rep:?}");
}
