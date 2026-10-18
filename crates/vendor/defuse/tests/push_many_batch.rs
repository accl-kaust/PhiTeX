//! `Engine::push_many` past its batch threshold (static facts taken on
//! several threads, nodes linked under one writer) builds the same program
//! as the pushes one by one: equal outputs, cold and after an edit, with
//! scopes opened and closed among the nodes.

use std::sync::Arc;

use defuse::{Brackets, Ctx, Effect, Engine, Field, Locality, NodeId, Operation, Stop, ValueId, Values};

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "d" => vec![(v.field("x"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> Effect {
        match tag(v, a).as_str() {
            "{" => Effect::Scope(Brackets {
                closes: 0,
                opens: 1,
                before: false,
                kind: None,
                kind_pending: false,
            }),
            "}" => Effect::Scope(Brackets {
                closes: 1,
                opens: 0,
                before: true,
                kind: None,
                kind_pending: false,
            }),
            _ => Effect::None,
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "d" => {
                let prev = cx.use_field(v.field("x"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                let x = v.int(prev + v.as_int(a[1]).unwrap_or(0));
                cx.define(v.field("x"), x);
                Ok(x)
            }
            "r" => Ok(cx.use_field(v.field("x"))?.unwrap_or(ValueId::UNIT)),
            _ => Ok(ValueId::UNIT),
        }
    }
}

const N: usize = 500;

fn texts(eng: &Engine) -> Vec<Vec<ValueId>> {
    let v = eng.values();
    (0..N)
        .map(|i| match i % 9 {
            0 => vec![v.str("{")],
            8 => vec![v.str("}")],
            1 | 4 => vec![v.str("d"), v.int(i as i64)],
            _ => vec![v.str("r")],
        })
        .collect()
}

fn build(batch: bool) -> (Engine, Vec<NodeId>, defuse::OpId) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let ts = texts(&eng);
    let ns = if batch {
        eng.push_many(u, op, ts)
    } else {
        ts.into_iter().map(|t| eng.push(u, op, t)).collect()
    };
    (eng, ns, op)
}

fn outs(eng: &Engine, ns: &[NodeId]) -> Vec<Option<i64>> {
    ns.iter()
        .map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x)))
        .collect()
}

#[test]
fn a_batch_builds_what_the_pushes_build() {
    let (mut one, ons, oop) = build(false);
    let (mut many, mns, mop) = build(true);
    one.evaluate_sequential().expect("evaluate");
    many.evaluate_sequential().expect("evaluate");
    assert_eq!(outs(&one, &ons), outs(&many, &mns), "cold");
    assert!(many.check_invariants().is_empty(), "{:?}", many.check_invariants());
    for (eng, ns, op) in [(&mut one, &ons, oop), (&mut many, &mns, mop)] {
        let (d, k) = (eng.values().str("d"), eng.values().int(1000));
        eng.replace(ns[1], op, vec![d, k]);
        eng.evaluate_sequential().expect("evaluate");
    }
    assert_eq!(outs(&one, &ons), outs(&many, &mns), "after an edit");
    assert!(many.check_reads().is_empty(), "{:?}", many.check_reads());
}
