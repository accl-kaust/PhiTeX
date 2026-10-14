//! Phi's 10-node unit: `p1` defines `f`; `p2` and `p3` read `f` and
//! define two fields each (`a`, `b` and `c`, `d`); `c4` reads `a` and `c`;
//! five plain nodes follow. Prints the critical path and the chain of the
//! consumer with the producers' definition sets unbounded (`Any`, pending
//! effect and claim) and bounded (`Names`, no effect, no claim).

#![allow(clippy::many_single_char_names)]

use std::sync::Arc;

use defuse::{Claim, Ctx, Effect, Engine, Field, Locality, MayDefine, OpId, Operation, Stop, Value, ValueId, Values};

struct Op {
    bounded: bool,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    match a.first().map(|&x| v.get(x)).as_deref() {
        Some(Value::Str(s)) => s.to_string(),
        _ => String::new(),
    }
}

impl Operation for Op {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "p1" => Some(vec![(v.field("f"), Locality::Local)]),
            "p2" | "p3" => None,
            _ => Some(Vec::new()),
        }
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> MayDefine {
        match (self.bounded, tag(v, a).as_str()) {
            (true, "p2") => MayDefine::Names(vec![v.field("a"), v.field("b")]),
            (true, "p3") => MayDefine::Names(vec![v.field("c"), v.field("d")]),
            _ => MayDefine::Any,
        }
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> Effect {
        if !self.bounded && matches!(tag(v, a).as_str(), "p2" | "p3") {
            Effect::Pending
        } else {
            Effect::None
        }
    }
    fn claim(&self, a: &[ValueId], v: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if !self.bounded && matches!(tag(v, a).as_str(), "p2" | "p3") {
            Claim::Pending
        } else {
            Claim::None
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let t = tag(v, &a);
        match t.as_str() {
            "p1" => {
                cx.define(v.field("f"), v.int(1));
                Ok(ValueId::UNIT)
            }
            "p2" | "p3" => {
                let f = cx.use_field(v.field("f"))?;
                let file = cx.use_env(v.field(if t == "p2" { "file2" } else { "file3" }))?;
                let _ = file;
                let (x, y) = if t == "p2" { ("a", "b") } else { ("c", "d") };
                let k = f.unwrap_or(ValueId::UNIT);
                cx.define(v.field(x), k);
                cx.define(v.field(y), k);
                if !self.bounded {
                    cx.set_effect(Effect::None);
                    cx.claim(0);
                }
                Ok(ValueId::UNIT)
            }
            "c4" => {
                let x = cx.use_field(v.field("a"))?;
                let y = cx.use_field(v.field("c"))?;
                Ok(cx
                    .values()
                    .list(vec![x.unwrap_or(ValueId::UNIT), y.unwrap_or(ValueId::UNIT)]))
            }
            _ => Ok(ValueId::UNIT),
        }
    }
}

fn main() {
    for sched in ["seq", "greedy", "predicted", "all"] {
        println!("== {sched}");
        for bounded in [false, true] {
            let mut eng = Engine::new();
            let op = eng.register(Arc::new(Op { bounded }));
            eng.set_env_provider(Box::new(|v: &Values, f: Field| Some(Some(v.list(vec![f.0])))));
            let u = eng.new_unit();
            let mut ids = Vec::new();
            for t in ["p1", "p2", "p3", "c4", "n5", "n6", "n7", "n8", "n9", "n10"] {
                let a = vec![eng.values().str(t)];
                ids.push(eng.push(u, op, a));
            }
            let r = match sched {
                "seq" => eng.evaluate_sequential(),
                "greedy" => eng.evaluate(&mut defuse::Greedy::default(), &mut defuse::Sequential),
                "predicted" => eng.evaluate(
                    &mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 64),
                    &mut defuse::Sequential,
                ),
                _ => eng.evaluate(
                    &mut defuse::Greedy::speculative(defuse::Speculation::All, 64),
                    &mut defuse::Sequential,
                ),
            }
            .expect("evaluate");
            println!(
                "bounded {bounded}: critical path {}  runs {} stops {}",
                eng.critical_path(),
                r.runs,
                r.stops
            );
            let names = ["p1", "p2", "p3", "c4", "n5", "n6", "n7", "n8", "n9", "n10"];
            let name = |n: defuse::NodeId| ids.iter().position(|&i| i == n).map_or("?", |k| names[k]);
            for (n, h, c) in eng.critical_chain_heights() {
                println!("  {} at {h}: {c:?}", name(n));
            }
        }
    }
}
