//! A forward window read (`Ctx::acc_next`) ending at a contributor still
//! sees that contributor's change after its unit was renumbered: unit
//! [d, r, a, b, x]; `r` walks `c`'s contributors with `acc_next` and
//! outputs what it folded; `a` contributes its argument, `b` the value of
//! `v`, which `d` defines. Nodes inserted right after `a` until the unit
//! is renumbered move `b`; then `d` changes `v`, `b`'s contribution with
//! it (its text the same), and `r` must see it.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, Monoid, Operation, Stop, ValueId, Values};

struct Sum;

impl Monoid for Sum {
    fn empty(&self, v: &Values) -> ValueId {
        v.int(0)
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        v.int(v.as_int(a).unwrap() + v.as_int(b).unwrap())
    }
}

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "d" => Some(vec![(v.field("v"), Locality::Global)]),
            _ => Some(Vec::new()),
        }
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        match tag(v, a).as_str() {
            "c" | "cv" => None,
            _ => Some(Vec::new()),
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let c = v.field("c");
        match tag(v, &a).as_str() {
            "r" => {
                // (Each contributor's own value: the fold up to it.)
                let mut seen = Vec::new();
                let mut after = None;
                while let Some((x, fold)) = cx.acc_next(c, after)? {
                    seen.push(fold);
                    after = Some(x);
                }
                Ok(v.list(seen))
            }
            "c" => {
                cx.contribute(c, a[1]);
                Ok(ValueId::UNIT)
            }
            "cv" => {
                let x = cx.use_field(v.field("v"))?.unwrap_or(v.int(0));
                cx.contribute(c, x);
                Ok(ValueId::UNIT)
            }
            "d" => {
                cx.define_with(v.field("v"), a[1], Locality::Global);
                Ok(ValueId::UNIT)
            }
            _ => Ok(ValueId::UNIT),
        }
    }
}

#[test]
fn a_window_to_a_contributor_holds_through_a_renumbering() {
    for seq in [true, false] {
        let mut eng = Engine::new();
        eng.declare_accumulator(eng.values().field("c"), Arc::new(Sum));
        let op = eng.register(Arc::new(Ops));
        let u = eng.new_unit();
        let args = |eng: &Engine, t: &str, x: i64| vec![eng.values().str(t), eng.values().int(x)];
        let ad = args(&eng, "d", 2);
        let d = eng.push(u, op, ad);
        let a0 = args(&eng, "r", 0);
        let r = eng.push(u, op, a0);
        let a1 = args(&eng, "c", 1);
        let a = eng.push(u, op, a1);
        let a2 = args(&eng, "cv", 0);
        eng.push(u, op, a2);
        let a3 = args(&eng, "x", 0);
        eng.push(u, op, a3);
        let run = |eng: &mut Engine| {
            if seq {
                eng.evaluate_sequential().expect("evaluate");
            } else {
                eng.evaluate(&mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 64), &mut phitex_defuse::Sequential).expect("evaluate");
            }
        };
        let got = |eng: &Engine| eng.output(r).map(|x| eng.values().with_list(x, |e| e.iter().map(|&y| eng.values().as_int(y).unwrap()).collect::<Vec<_>>()).unwrap());
        run(&mut eng);
        assert_eq!(got(&eng), Some(vec![1, 2]), "seq {seq}");
        // Right after `a`, each in the gap the last left: the unit is
        // renumbered, `b` moves.
        for _ in 0..64 {
            let n = args(&eng, "n", 0);
            eng.insert_after(a, op, n);
        }
        run(&mut eng);
        let d5 = args(&eng, "d", 5);
        eng.replace(d, op, d5);
        run(&mut eng);
        assert_eq!(got(&eng), Some(vec![1, 5]), "seq {seq}");
        assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    }
}
