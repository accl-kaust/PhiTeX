//! The engine's behaviour where the TeX client needs it, on a toy client
//! (no TeX): each test is self-contained, to forward as an engine report.

use std::sync::Arc;

use phitex_defuse::{Claim, Ctx, Engine, Field, Locality, OpId, Operation, Stop, Value, ValueId, Values};

/// `["p"]` unfolds a child `["c"]`; `["c"]` outputs the text of its
/// first following sibling (or "none") and claims it; `["s", x]` outputs x.
struct Toy;

impl Operation for Toy {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn claim(&self, a: &[ValueId], v: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if tag(v, a) == "c" { Claim::Pending } else { Claim::None }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "p" => {
                let op = OP.get().copied().unwrap();
                cx.child(op, vec![v.str("c")]);
                Ok(ValueId::UNIT)
            }
            "c" => {
                let f = cx.following(0);
                cx.claim(usize::from(f.is_some()));
                Ok(f.map_or_else(|| v.str("none"), |(_, xs)| xs.get(1).copied().unwrap_or(ValueId::UNIT)))
            }
            _ => Ok(a.get(1).copied().unwrap_or(ValueId::UNIT)),
        }
    }
}

static OP: std::sync::OnceLock<OpId> = std::sync::OnceLock::new();

fn tag(v: &Values, a: &[ValueId]) -> String {
    a.first().map_or_else(String::new, |&x| match &*v.get(x) {
        Value::Str(s) => s.to_string(),
        _ => String::new(),
    })
}

/// A child's claim reaches its parent's following sibling (`Ctx::child`
/// splices the child inline after the parent): the TeX client needs this
/// for a macro body whose last token takes its argument from the text
/// after the call.
#[test]
fn a_child_claims_its_parents_following_sibling() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Toy));
    let _ = OP.set(op);
    let u = eng.new_unit();
    let p = eng.push(u, op, vec![eng.values().str("p")]);
    let _s = eng.push(u, op, vec![eng.values().str("s"), eng.values().str("sibling")]);
    eng.evaluate_sequential().expect("evaluate");
    let c = eng.children(p)[0];
    let got = eng.output(c).map(|x| eng.values().render(x));
    assert_eq!(got.as_deref(), Some("\"sibling\""), "the child saw no following sibling");
}

/// `["c"]` claims its following sibling (a pending claim); `["d"]` has a
/// dynamic definition set and defines `x`; `["r"]` reads `x`.
struct Toy2;

impl Operation for Toy2 {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        if tag(v, a) == "d" { None } else { Some(Vec::new()) }
    }
    fn claim(&self, a: &[ValueId], v: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if matches!(tag(v, a).as_str(), "r" | "f") { Claim::None } else { Claim::Pending }
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> phitex_defuse::Effect {
        if matches!(tag(v, a).as_str(), "r" | "f") { phitex_defuse::Effect::None } else { phitex_defuse::Effect::Pending }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "c" => {
                cx.claim(1);
                cx.set_effect(phitex_defuse::Effect::None);
                Ok(ValueId::UNIT)
            }
            "d" => {
                cx.claim(0);
                cx.set_effect(phitex_defuse::Effect::None);
                cx.define(v.field("x"), v.int(1));
                Ok(ValueId::UNIT)
            }
            "f" => {
                let _ = cx.use_field(v.field("x"))?;
                cx.acc_range(v.field("acc"), None)
            }
            _ => Ok(cx.use_field(v.field("x"))?.unwrap_or_else(|| v.str("none"))),
        }
    }
}

/// A node parked on a node that a claim then makes inert is woken: here
/// `r` may park on `d` (pending definitions) before `c` claims `d`.
/// Seen: `EvalError::Cycle([r])`.
#[test]
fn a_reader_parked_on_a_node_that_becomes_claimed_is_woken() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Toy2));
    let u = eng.new_unit();
    let s = |e: &Engine, t: &str| e.values().str(t);
    let a = s(&eng, "c");
    eng.push(u, op, vec![a]);
    let a = s(&eng, "d");
    eng.push(u, op, vec![a]);
    let a = s(&eng, "r");
    let r = eng.push(u, op, vec![a]);
    let a = s(&eng, "f");
    eng.push(u, op, vec![a]);
    let acc = eng.values().field("acc");
    eng.declare_accumulator(acc, Arc::new(phitex_defuse::SeqMonoid));
    eng.evaluate_sequential().expect("evaluate");
    let got = eng.output(r).map(|x| eng.values().render(x));
    assert_eq!(got.as_deref(), Some("\"none\""));
}
