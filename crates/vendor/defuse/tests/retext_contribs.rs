//! A node given a new text before it ran contributes what the new text
//! declares (`Operation::static_contribs`), not the old: unit
//! [d, f0, x, fs, r]; `d` defines `count:0`; `f0` (undeclared) and `fs`
//! (declared) add 1 to `acc:count:0`; each node reads the fold of
//! `acc:count:0` after `count:0`'s definition. `x` is made with one text
//! and given another (`Engine::replace`) before the evaluation: from one
//! that declares nothing to one that declares and adds 1, and from one
//! that declares and adds 1 to one that declares nothing and adds nothing.

use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, Monoid, Operation, Stop, ValueId, Values};

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
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "d" => Some(vec![(v.field("count:0"), Locality::Global)]),
            _ => Some(Vec::new()),
        }
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        match tag(v, a).as_str() {
            "fs" => Some(vec![(v.field("acc:count:0"), plus_one(v))]),
            "f0" => None,
            _ => Some(Vec::new()),
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let t = tag(v, &a);
        if t == "d" {
            cx.define_with(v.field("count:0"), v.int(1), Locality::Global);
            return Ok(ValueId::UNIT);
        }
        let (d, fold) = cx.acc_range_after_def(v.field("acc:count:0"), v.field("count:0"))?;
        // (Adds 1 but "r" and "q", which add nothing.)
        if t != "r" && t != "q" {
            cx.contribute(v.field("acc:count:0"), plus_one(v));
        }
        Ok(v.int(d.and_then(|d| v.as_int(d)).unwrap_or(0) * 100 + map_of(v, fold).1))
    }
}

#[test]
fn a_new_text_contributes_what_it_declares() {
    for (from, to, want) in [("fz", "fs", [100, 101, 102, 103]), ("fs", "q", [100, 101, 101, 102])] {
        for seq in [true, false] {
            let mut eng = Engine::new();
            eng.declare_accumulator_prefix("acc:count:", Arc::new(Affine));
            let op = eng.register(Arc::new(Ops));
            let u = eng.new_unit();
            let mut ids = Vec::new();
            for t in ["d", "f0", from, "fs", "r"] {
                let a = vec![eng.values().str(t)];
                ids.push(eng.push(u, op, a));
            }
            let a = vec![eng.values().str(to)];
            eng.replace(ids[2], op, a);
            if seq {
                eng.evaluate_sequential().expect("evaluate");
            } else {
                eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 64), &mut defuse::Sequential).expect("evaluate");
            }
            let got: Vec<i64> = ids[1..].iter().map(|&x| eng.output(x).and_then(|x| eng.values().as_int(x)).unwrap_or(-1)).collect();
            assert_eq!(got, want, "{from} -> {to}, seq {seq}");
            assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
        }
    }
}
