//! The engine's behaviour where the TeX client needs it, on a toy client
//! (no TeX): each test is self-contained, to forward as an engine report.

#![allow(clippy::many_single_char_names)]

use std::sync::Arc;

use phitex_defuse::{
    Claim, Ctx, Engine, Field, Locality, OpId, Operation, Stop, Value, ValueId, Values,
};

/// `["p"]` unfolds a child `["c"]`; `["c"]` outputs the text of its
/// first following sibling (or "none") and claims it; `["s", x]` outputs x.
struct Toy;

impl Operation for Toy {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn claim(
        &self,
        a: &[ValueId],
        v: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        if tag(v, a) == "c" {
            Claim::Pending
        } else {
            Claim::None
        }
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
                Ok(f.map_or_else(
                    || v.str("none"),
                    |(_, xs)| xs.get(1).copied().unwrap_or(ValueId::UNIT),
                ))
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
    let _s = eng.push(
        u,
        op,
        vec![eng.values().str("s"), eng.values().str("sibling")],
    );
    eng.evaluate_sequential().expect("evaluate");
    let c = eng.children(p)[0];
    let got = eng.output(c).map(|x| eng.values().render(x));
    assert_eq!(
        got.as_deref(),
        Some("\"sibling\""),
        "the child saw no following sibling"
    );
}

/// `["c"]` claims its following sibling (a pending claim); `["d"]` has a
/// dynamic definition set and defines `x`; `["r"]` reads `x`.
struct Toy2;

impl Operation for Toy2 {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        if tag(v, a) == "d" {
            None
        } else {
            Some(Vec::new())
        }
    }
    fn claim(
        &self,
        a: &[ValueId],
        v: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        if matches!(tag(v, a).as_str(), "r" | "f") {
            Claim::None
        } else {
            Claim::Pending
        }
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> phitex_defuse::Effect {
        if matches!(tag(v, a).as_str(), "r" | "f") {
            phitex_defuse::Effect::None
        } else {
            phitex_defuse::Effect::Pending
        }
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

/// `["c"]` claims 3 following siblings when the first one's text is "x",
/// else 1, and then unfolds a child `["k"]`; `["k"]` and `["s", t]`
/// contribute their text to `acc`; `["f"]` reads `acc`.
struct Toy3;

impl Operation for Toy3 {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn claim(
        &self,
        a: &[ValueId],
        v: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        if tag(v, a) == "c" {
            Claim::Pending
        } else {
            Claim::None
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let acc = v.field("acc");
        match tag(v, &a).as_str() {
            "c" => {
                let first = cx.following(0).and_then(|(_, xs)| xs.get(1).copied());
                let n = if first.is_some_and(|x| matches!(&*v.get(x), Value::Str(s) if &**s == "x"))
                {
                    3
                } else {
                    1
                };
                for i in 0..n {
                    let _ = cx.following(i);
                }
                cx.claim(n);
                if n == 1 {
                    let op = OP3.get().copied().unwrap();
                    cx.child(op, vec![v.str("k")]);
                }
                Ok(ValueId::UNIT)
            }
            "k" => {
                cx.contribute(acc, v.seq_unit(v.str("k")));
                Ok(ValueId::UNIT)
            }
            "f" => cx.acc_range(acc, None),
            _ => {
                let t = a.get(1).copied().unwrap_or(ValueId::UNIT);
                cx.contribute(acc, v.seq_unit(t));
                Ok(ValueId::UNIT)
            }
        }
    }
}

static OP3: std::sync::OnceLock<OpId> = std::sync::OnceLock::new();

fn toy3_unit(
    eng: &mut Engine,
    op: OpId,
    x: &str,
) -> (Vec<phitex_defuse::NodeId>, phitex_defuse::NodeId) {
    let u = eng.new_unit();
    let mut ns = Vec::new();
    let c = eng.values().str("c");
    ns.push(eng.push(u, op, vec![c]));
    for t in [x, "y", "z", "w"] {
        let a = vec![eng.values().str("s"), eng.values().str(t)];
        ns.push(eng.push(u, op, a));
    }
    let f = eng.values().str("f");
    let f = eng.push(u, op, vec![f]);
    (ns, f)
}

/// A claimer's claim shrinks after an edit (3 siblings → 1) and it
/// unfolds a new child: the child's
/// place in text order follows the new claim, so its contribution comes
/// before the siblings released from the claim, as in a cold evaluation
/// of the edited unit.
#[test]
fn a_child_moves_back_when_its_parents_claim_shrinks() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Toy3));
    let _ = OP3.set(op);
    let acc = eng.values().field("acc");
    eng.declare_accumulator(acc, Arc::new(phitex_defuse::SeqMonoid));
    let (ns, f) = toy3_unit(&mut eng, op, "x");
    eng.evaluate_sequential().expect("evaluate");
    let a = vec![eng.values().str("s"), eng.values().str("x2")];
    eng.replace(ns[1], op, a);
    eng.evaluate_sequential().expect("evaluate after the edit");
    let got = eng.output(f).map(|x| eng.values().render(x));

    let mut cold = Engine::new();
    let op2 = cold.register(Arc::new(Toy3));
    let acc2 = cold.values().field("acc");
    cold.declare_accumulator(acc2, Arc::new(phitex_defuse::SeqMonoid));
    let (_, f2) = toy3_unit(&mut cold, op2, "x2");
    cold.evaluate_sequential().expect("cold evaluate");
    let want = cold.output(f2).map(|x| cold.values().render(x));
    assert_eq!(got, want, "edited order differs from cold");
}

/// `["p", t]` unfolds children `["c"]` and `["s", t]`; `["c"]` claims its
/// following sibling and outputs that sibling's text; `["s", t]`
/// contributes `t` to `acc`; `["f"]` reads `acc`.
struct Toy4;

impl Operation for Toy4 {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn claim(
        &self,
        a: &[ValueId],
        v: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        if tag(v, a) == "c" {
            Claim::Pending
        } else {
            Claim::None
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let acc = v.field("acc");
        match tag(v, &a).as_str() {
            "p" => {
                let op = OP4.get().copied().unwrap();
                cx.child(op, vec![v.str("c")]);
                let t = a.get(1).copied().unwrap_or(ValueId::UNIT);
                cx.child(op, vec![v.str("s"), t]);
                Ok(ValueId::UNIT)
            }
            "c" => {
                let f = cx.following(0);
                cx.claim(usize::from(f.is_some()));
                Ok(f.map_or_else(
                    || v.str("none"),
                    |(_, xs)| xs.get(1).copied().unwrap_or(ValueId::UNIT),
                ))
            }
            "f" => cx.acc_range(acc, None),
            _ => {
                let t = a.get(1).copied().unwrap_or(ValueId::UNIT);
                cx.contribute(acc, v.seq_unit(t));
                Ok(ValueId::UNIT)
            }
        }
    }
}

static OP4: std::sync::OnceLock<OpId> = std::sync::OnceLock::new();

fn toy4_unit(
    eng: &mut Engine,
    op: OpId,
    t: &str,
) -> (phitex_defuse::NodeId, phitex_defuse::NodeId) {
    let u = eng.new_unit();
    let a = vec![eng.values().str("p"), eng.values().str(t)];
    let p = eng.push(u, op, a);
    let f = eng.values().str("f");
    let f = eng.push(u, op, vec![f]);
    (p, f)
}

/// A parent's edit replaces the child that its other child claims: the
/// claimer must see the new sibling (re-run, keep the claim), so the
/// replaced child stays inert, as in a cold evaluation. Seen: the claimer
/// keeps its old output and the new sibling runs unclaimed.
#[test]
fn a_claimer_reruns_when_the_sibling_it_claims_is_replaced() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Toy4));
    let _ = OP4.set(op);
    let acc = eng.values().field("acc");
    eng.declare_accumulator(acc, Arc::new(phitex_defuse::SeqMonoid));
    let (p, f) = toy4_unit(&mut eng, op, "old");
    eng.evaluate_sequential().expect("evaluate");
    let a = vec![eng.values().str("p"), eng.values().str("new")];
    eng.replace(p, op, a);
    eng.evaluate_sequential().expect("evaluate after the edit");
    let c = eng.children(p)[0];
    let got = (
        eng.output(c).map(|x| eng.values().render(x)),
        eng.output(f).map(|x| eng.values().render(x)),
    );

    let mut cold = Engine::new();
    let op2 = cold.register(Arc::new(Toy4));
    let acc2 = cold.values().field("acc");
    cold.declare_accumulator(acc2, Arc::new(phitex_defuse::SeqMonoid));
    let (p2, f2) = toy4_unit(&mut cold, op2, "new");
    cold.evaluate_sequential().expect("cold evaluate");
    let c2 = cold.children(p2)[0];
    let want = (
        cold.output(c2).map(|x| cold.values().render(x)),
        cold.output(f2).map(|x| cold.values().render(x)),
    );
    assert_eq!(
        got, want,
        "(claimer's output, acc) after the edit differ from cold"
    );
}
