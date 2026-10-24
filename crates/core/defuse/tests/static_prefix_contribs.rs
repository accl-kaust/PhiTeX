//! A contribution a node declares (`Operation::static_contribs`) to an
//! accumulator declared by prefix (`Engine::declare_accumulator_prefix`)
//! is folded as any other: unit [f0, f1, f2, r]; `f0` contributes `+1` to
//! `acc:count:0` undeclared, `f1` and `f2` declare and contribute `+1`;
//! `r` folds `acc:count:0` after `count:0`'s definition (none): `+3`,
//! the accumulator declared before the nodes are made or after.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, Monoid, Operation, Stop, ValueId, Values};

/// Affine maps `x -> a x + c` as `[a, c]`, composed in position order.
struct Affine;

fn map_of(v: &Values, x: ValueId) -> (i64, i64) {
    v.with_list(x, |e| (v.as_int(e[0]).unwrap(), v.as_int(e[1]).unwrap())).unwrap_or((1, 0))
}

impl Monoid for Affine {
    fn empty(&self, v: &Values) -> ValueId {
        v.list(vec![v.int(1), v.int(0)])
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        let ((aa, ac), (ba, bc)) = (map_of(v, a), map_of(v, b));
        v.list(vec![v.int(aa * ba), v.int(ba * ac + bc)])
    }
}

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

fn plus_one(v: &Values) -> ValueId {
    v.list(vec![v.int(1), v.int(1)])
}

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        match tag(v, a).as_str() {
            "fs" => Some(vec![(v.field("acc:count:0"), plus_one(v))]),
            "r" => Some(Vec::new()),
            _ => None,
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "r" => {
                let (_, fold) = cx.acc_range_after_def(v.field("acc:count:0"), v.field("count:0"))?;
                Ok(v.int(map_of(v, fold).1))
            }
            _ => {
                cx.contribute(v.field("acc:count:0"), plus_one(v));
                Ok(ValueId::UNIT)
            }
        }
    }
}

#[test]
fn declared_contributions_to_a_prefix_accumulator_are_folded() {
    for (seq, late) in [(true, false), (false, false), (true, true), (false, true)] {
        let mut eng = Engine::new();
        if !late {
            eng.declare_accumulator_prefix("acc:count:", Arc::new(Affine));
        }
        let op = eng.register(Arc::new(Ops));
        let u = eng.new_unit();
        for t in ["f0", "fs", "fs"] {
            let a = vec![eng.values().str(t)];
            eng.push(u, op, a);
        }
        let r = vec![eng.values().str("r")];
        let r = eng.push(u, op, r);
        // (`late`: declared after the nodes are made.)
        if late {
            eng.declare_accumulator_prefix("acc:count:", Arc::new(Affine));
        }
        if seq {
            eng.evaluate_sequential().expect("evaluate");
        } else {
            eng.evaluate(&mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 64), &mut phitex_defuse::Sequential).expect("evaluate");
        }
        assert_eq!(eng.output(r).and_then(|x| eng.values().as_int(x)), Some(3), "seq {seq} late {late}");
        assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    }
}
