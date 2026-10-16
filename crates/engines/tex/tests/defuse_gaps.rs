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

/// Scopes: `["O"]` opens one with itself inside it (`before`); `["C"]`
/// closes one and opens one after itself; `["E"]` closes one;
/// `["d", n]` defines `x` locally as n; `["r"]` reads `x`.
struct Toy5;

impl Operation for Toy5 {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "d" {
            vec![(v.field("x"), Locality::Local)]
        } else {
            Vec::new()
        })
    }
    fn claim(
        &self,
        _: &[ValueId],
        _: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        Claim::None
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> phitex_defuse::Effect {
        use phitex_defuse::{Brackets, Effect};
        match tag(v, a).as_str() {
            "O" => Effect::Scope(Brackets {
                closes: 0,
                opens: 1,
                before: true,
                kind: None,
                kind_pending: false,
            }),
            "C" => Effect::Scope(Brackets {
                closes: 1,
                opens: 1,
                before: false,
                kind: None,
                kind_pending: false,
            }),
            "E" => Effect::Close,
            _ => Effect::None,
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "d" => {
                cx.define(v.field("x"), a.get(1).copied().unwrap_or(ValueId::UNIT));
                Ok(ValueId::UNIT)
            }
            "r" => Ok(cx.use_field(v.field("x"))?.unwrap_or_else(|| v.str("none"))),
            _ => Ok(ValueId::UNIT),
        }
    }
}

/// A definition in a group reaches a group that a node closes and
/// reopens inside it (`closes: 1, opens: 1` after the node): `r1` sees
/// `x = 1` like `r0`. Seen: `r1` sees the value from outside the group.
#[test]
fn a_reopened_scope_sees_the_enclosing_definitions() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Toy5));
    let u = eng.new_unit();
    let node = |e: &mut Engine, t: &str, n: Option<i64>| {
        let mut a = vec![e.values().str(t)];
        if let Some(n) = n {
            a.push(e.values().int(n));
        }
        e.push(u, op, a)
    };
    node(&mut eng, "d", Some(0));
    node(&mut eng, "O", None);
    node(&mut eng, "d", Some(1));
    node(&mut eng, "O", None);
    let r0 = node(&mut eng, "r", None);
    node(&mut eng, "C", None);
    let r1 = node(&mut eng, "r", None);
    node(&mut eng, "E", None);
    node(&mut eng, "E", None);
    eng.evaluate_sequential().expect("evaluate");
    let out = |e: &Engine, n| e.output(n).map(|x| e.values().render(x));
    assert_eq!(out(&eng, r0).as_deref(), Some("1"));
    assert_eq!(
        out(&eng, r1).as_deref(),
        Some("1"),
        "the reopened scope lost the enclosing definition"
    );
}

/// `["i"]` has a pending definition set (`defs` None, `may_define` Any)
/// and reads the environment field `file`; when it has it, it defines
/// `x` to it. `["r"]` reads `x` and outputs it (or "none").
struct ToyFile;

impl Operation for ToyFile {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        if tag(v, a) == "i" {
            None
        } else {
            Some(Vec::new())
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) == "i" {
            if let Some(x) = cx.use_env(v.field("file"))? {
                cx.define(v.field("x"), x);
            }
            return Ok(ValueId::UNIT);
        }
        Ok(cx.use_field(v.field("x"))?.unwrap_or_else(|| v.str("none")))
    }
}

/// The TeX client's `\input` of a file the client has not supplied yet:
/// a node with a pending definition set whose environment field is
/// undefined on the first evaluation. Later readers must not run ahead
/// of it with what it would have defined missing (they wait, or re-run
/// once the field is supplied and it defines `x`).
#[test]
fn a_reader_waits_for_a_pending_node_whose_file_is_not_supplied() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(ToyFile));
    let u = eng.new_unit();
    let _i = eng.push(u, op, vec![eng.values().str("i")]);
    let r = eng.push(u, op, vec![eng.values().str("r")]);
    eng.add_root(r);
    eng.evaluate_sequential().expect("evaluate");
    let first = eng.output(r).map(|x| eng.values().render(x));
    let f = eng.values().field("file");
    let five = eng.values().int(5);
    eng.set_environment(vec![(f, five)]);
    eng.evaluate_sequential().expect("evaluate");
    let second = eng.output(r).map(|x| eng.values().render(x));
    assert_eq!(
        second.as_deref(),
        Some("5"),
        "r did not re-run after i defined x"
    );
    assert_eq!(
        first, None,
        "r ran ahead of i on the first evaluation (saw {first:?}) while i's file was undefined"
    );
}

/// As [`ToyFile`], with `["i"]`'s claim and scope effect pending (known
/// when it runs), as every TeX command node has them.
struct ToyFilePending;

impl Operation for ToyFilePending {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        ToyFile.defs(a, v)
    }
    fn claim(
        &self,
        a: &[ValueId],
        v: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        if tag(v, a) == "i" {
            Claim::Pending
        } else {
            Claim::None
        }
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> phitex_defuse::Effect {
        if tag(v, a) == "i" {
            phitex_defuse::Effect::Pending
        } else {
            phitex_defuse::Effect::None
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let a = cx.args();
        if tag(cx.values(), &a) == "i" {
            cx.claim(0);
            cx.set_effect(phitex_defuse::Effect::None);
        }
        ToyFile.run(cx)
    }
}

/// A node waiting on an environment field with its claim pending: the
/// evaluation must end with it stalled (`Report::env_stalled`), not as a
/// cycle; supplying the field finishes it.
#[test]
fn an_env_wait_with_a_pending_claim_is_not_a_cycle() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(ToyFilePending));
    let u = eng.new_unit();
    let _i = eng.push(u, op, vec![eng.values().str("i")]);
    let r = eng.push(u, op, vec![eng.values().str("r")]);
    eng.add_root(r);
    let rep = eng.evaluate_sequential();
    assert!(rep.is_ok(), "an env wait ended as {:?}", rep.err());
    let f = eng.values().field("file");
    let five = eng.values().int(5);
    eng.set_environment(vec![(f, five)]);
    eng.evaluate_sequential().expect("evaluate");
    let got = eng.output(r).map(|x| eng.values().render(x));
    assert_eq!(got.as_deref(), Some("5"));
}

/// As [`ToyFile`]; `["a"]` reads the fold of the accumulator `acc` over
/// the unit (as the TeX client's DVI node reads the pages).
struct ToyFileAcc;

impl Operation for ToyFileAcc {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        ToyFile.defs(a, v)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let a = cx.args();
        let v = cx.values();
        if tag(v, &a) == "a" {
            return cx.acc_range(v.field("acc"), None);
        }
        ToyFile.run(cx)
    }
}

/// A node waiting on an environment field before an accumulator reader:
/// the evaluation ends with both stalled (`Report::env_stalled`), not as
/// a cycle.
#[test]
fn an_env_wait_before_an_accumulator_reader_is_not_a_cycle() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(ToyFileAcc));
    let acc = eng.values().field("acc");
    eng.declare_accumulator(acc, Arc::new(phitex_defuse::SeqMonoid));
    let u = eng.new_unit();
    let _i = eng.push(u, op, vec![eng.values().str("i")]);
    let a = eng.push(u, op, vec![eng.values().str("a")]);
    eng.add_root(a);
    let rep = eng.evaluate_sequential();
    assert!(rep.is_ok(), "an env wait ended as {:?}", rep.err());
}

/// An evaluation that ends with many nodes stalled behind one node waiting
/// for an environment field costs time linear in them: 15 000 readers
/// behind it evaluate in well under a second (the TeX client's plain
/// prelude, one environment field per round, took about 30 s a round).
#[test]
fn many_readers_stalled_behind_an_env_wait_are_cheap() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(ToyFile));
    let u = eng.new_unit();
    let _i = eng.push(u, op, vec![eng.values().str("i")]);
    let mut last = None;
    for _ in 0..15_000 {
        last = Some(eng.push(u, op, vec![eng.values().str("r")]));
    }
    eng.add_root(last.unwrap());
    let t0 = std::time::Instant::now();
    eng.evaluate_sequential().expect("evaluate");
    let first = t0.elapsed();
    let t1 = std::time::Instant::now();
    let g = eng.values().field("other");
    let one = eng.values().int(1);
    eng.set_environment(vec![(g, one)]);
    eng.evaluate_sequential().expect("evaluate");
    let second = t1.elapsed();
    assert!(
        second.as_secs_f64() < 1.0,
        "a round with nothing new took {second:?} (the first {first:?})"
    );
}

/// Every node has its claim and scope effect pending (known when it
/// runs), reads `x`, and claims nothing; `["i"]` first waits for the
/// environment field `file`.
struct ToyPendingAll;

impl Operation for ToyPendingAll {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        ToyFile.defs(a, v)
    }
    fn claim(
        &self,
        _: &[ValueId],
        _: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        Claim::Pending
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> phitex_defuse::Effect {
        phitex_defuse::Effect::Pending
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.claim(0);
        cx.set_effect(phitex_defuse::Effect::None);
        ToyFile.run(cx)
    }
}

/// The cost of a round that ends with 15 000 nodes stalled behind one
/// environment wait, when every node's claim and scope effect are
/// pending: a round that supplies an unrelated field (nothing to run)
/// should cost well under a second.
#[test]
fn stalled_nodes_with_pending_claims_and_effects_are_cheap() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(ToyPendingAll));
    let u = eng.new_unit();
    let _i = eng.push(u, op, vec![eng.values().str("i")]);
    let mut last = None;
    for _ in 0..15_000 {
        last = Some(eng.push(u, op, vec![eng.values().str("r")]));
    }
    eng.add_root(last.unwrap());
    let t0 = std::time::Instant::now();
    eng.evaluate_sequential().expect("evaluate");
    let first = t0.elapsed();
    let mut times = Vec::new();
    for k in 0..3 {
        let g = eng.values().field(&format!("other{k}"));
        let one = eng.values().int(1);
        eng.set_environment(vec![(g, one)]);
        let t = std::time::Instant::now();
        eng.evaluate_sequential().expect("evaluate");
        times.push(t.elapsed());
    }
    eprintln!("first {first:?}, then {times:?}");
    assert!(
        times.iter().all(|t| t.as_secs_f64() < 1.0),
        "rounds with nothing to run took {times:?} (the first {first:?})"
    );
}

/// `["w"]` waits for an environment field whose name it makes as it
/// runs (no one else holds the name's value).
struct ToyFreshName;

impl Operation for ToyFreshName {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        // enough garbage for the evaluation's value sweep to run
        for k in 0..20_000 {
            let _ = v.str(&format!("garbage-{k}"));
        }
        let name = format!("fresh-{}", 40 + 2);
        cx.use_env(v.field(&name))?;
        Ok(ValueId::UNIT)
    }
}

/// The fields `Report::env_waits` names can be read back by name after
/// the evaluation (their name values are not swept with the run's
/// garbage; the second evaluation is the first that sweeps).
#[test]
fn env_wait_field_names_survive_the_evaluation() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(ToyFreshName));
    let u = eng.new_unit();
    let first = eng.push(u, eng.open_op(), Vec::new());
    let _ = first;
    eng.evaluate_sequential().expect("evaluate");
    let w = eng.push(u, op, Vec::new());
    eng.add_root(w);
    let rep = eng.evaluate_sequential().expect("evaluate");
    let (_, f) = rep.env_waits.first().copied().expect("a wait");
    let name = eng.values().render(f.0);
    assert_eq!(name, "\"fresh-42\"");
}

/// `["w", n]` does one step of work and leaves the rest, `["w", n-1]`,
/// for after itself. The rest is not nested in the step: it belongs at
/// the step's own depth, as its next sibling. With `Ctx::child` the only
/// way to leave it, every step is one level deeper than the one before,
/// so a chain of 5000 steps (a long loop, a long file) would trip
/// `Limits::unfold_depth` (4096) although nothing is nested.
/// `Ctx::follow` leaves the continuation at the node's own depth.
struct Walk;

static WALK: std::sync::OnceLock<OpId> = std::sync::OnceLock::new();

impl Operation for Walk {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn claim(
        &self,
        _: &[ValueId],
        _: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        Claim::None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let n = match a.get(1).map(|&x| v.get(x)).as_deref() {
            Some(Value::Int(n)) => *n,
            _ => 0,
        };
        if n > 0 {
            let op = WALK.get().copied().unwrap();
            cx.follow(op, vec![v.str("w"), v.int(n - 1)]);
        }
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_continuation_stays_at_its_nodes_depth() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Walk));
    let _ = WALK.set(op);
    eng.set_limits(phitex_defuse::Limits {
        unfold_depth: 4096,
        ..phitex_defuse::Limits::default()
    });
    let u = eng.new_unit();
    let v = eng.values();
    let args = vec![v.str("w"), v.int(5000)];
    eng.push(u, op, args);
    eng.evaluate_sequential()
        .expect("a chain of 5000 steps is not nested");
}

/// `["i", kind, k]`: a node whose definitions, claim and scope effect are
/// known only when it runs (like every node of the text). Kinds: `d`
/// reads `x` and the contributions to `acc` before it, defines `x` and
/// contributes to `acc`; `o`/`e` open/close a
/// scope; `c` claims its following sibling; `u` unfolds two `d`
/// children; `r` reads the loop-back field `lb`; `p` publishes `lb`.
struct Item;

static ITEM: std::sync::OnceLock<OpId> = std::sync::OnceLock::new();

fn item_text(n: usize, salt: i64) -> Vec<(String, i64)> {
    (0..n)
        .map(|i| {
            let kind = if i == n - 1 {
                "p"
            } else if i % 10 == 0 {
                "o"
            } else if i % 10 == 9 {
                "e"
            } else if i % 50 == 5 {
                "c"
            } else if i % 25 == 7 {
                "u"
            } else if i == 3 || i == n / 2 {
                "r"
            } else if i % 7 == 3 {
                "f"
            } else {
                "d"
            };
            (kind.to_owned(), i64::try_from(i).unwrap_or(0) + salt)
        })
        .collect()
}

impl Operation for Item {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> phitex_defuse::Effect {
        phitex_defuse::Effect::Pending
    }
    fn claim(
        &self,
        _: &[ValueId],
        _: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        Claim::Pending
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let kind = match a.get(1).map(|&x| v.get(x)).as_deref() {
            Some(Value::Str(s)) => s.to_string(),
            _ => String::new(),
        };
        let k = match a.get(2).map(|&x| v.get(x)).as_deref() {
            Some(Value::Int(n)) => *n,
            _ => 0,
        };
        let x = v.field("x");
        let mut effect = phitex_defuse::Effect::None;
        let mut claim = 0;
        match kind.as_str() {
            "d" => {
                let old = cx.use_field(x)?.and_then(|y| v.as_int(y)).unwrap_or(0);
                // the contributions so far, from the unit's start
                let before = cx.acc_range(v.field("acc"), None)?;
                let m = i64::try_from(v.seq_to_vec(before).len()).unwrap_or(0);
                cx.define(x, v.int(old.wrapping_mul(31).wrapping_add(k + m)));
                let s = v.seq_unit(v.int(k));
                cx.contribute(v.field("acc"), s);
            }
            "o" => effect = phitex_defuse::Effect::Open,
            "e" => effect = phitex_defuse::Effect::Close,
            "c" => {
                claim = usize::from(cx.following(0).is_some());
                cx.define(v.field("y"), v.int(k));
                let op = ITEM.get().copied().unwrap();
                cx.follow(op, vec![v.str("i"), v.str("d"), v.int(k)]);
            }
            "f" => {
                let op = ITEM.get().copied().unwrap();
                let old = cx.use_field(x)?.and_then(|y| v.as_int(y)).unwrap_or(0);
                cx.child(op, vec![v.str("i"), v.str("d"), v.int(k)]);
                cx.follow(op, vec![v.str("i"), v.str("d"), v.int(old % 5)]);
                cx.follow(op, vec![v.str("i"), v.str("c"), v.int(k + 1)]);
            }
            "u" => {
                let op = ITEM.get().copied().unwrap();
                cx.child(op, vec![v.str("i"), v.str("d"), v.int(k)]);
                cx.child(op, vec![v.str("i"), v.str("d"), v.int(k + 1)]);
            }
            "r" => {
                let l = cx
                    .use_field(v.field("lb"))?
                    .and_then(|y| v.as_int(y))
                    .unwrap_or(0);
                cx.define(v.field("z"), v.int(l + k));
            }
            "p" => cx.publish(v.field("lb"), v.int(k % 3)),
            _ => {}
        }
        cx.set_effect(effect);
        cx.claim(claim);
        Ok(ValueId::UNIT)
    }
}

fn item_engine() -> (Engine, OpId) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Item));
    let _ = ITEM.set(op);
    let acc = eng.values().field("acc");
    eng.declare_accumulator(acc, Arc::new(phitex_defuse::SeqMonoid));
    let lb = eng.values().field("lb");
    let zero = eng.values().int(0);
    eng.declare_loopback(lb, zero);
    (eng, op)
}

fn item_args(v: &Values, kind: &str, k: i64) -> Vec<ValueId> {
    vec![v.str("i"), v.str(kind), v.int(k)]
}

/// An edit that replaces the whole text costs about what a cold
/// evaluation of the new text costs (a cold evaluation is an edit from
/// nothing).
#[test]
#[ignore = "engine gap: after the edit, reading the contributions so far costs 75x (46.8 s against 0.62 s cold)"]
fn replacing_the_whole_text_costs_a_cold_evaluation() {
    let n = 1000;
    let old = item_text(n, 0);
    let new = item_text(n, 7);
    // cold, new text
    let (mut cold, op) = item_engine();
    let u = cold.new_unit();
    for (kind, k) in &new {
        let a = item_args(cold.values(), kind, *k);
        cold.push(u, op, a);
    }
    let t0 = std::time::Instant::now();
    let rc = cold.evaluate_sequential().expect("cold");
    let tc = t0.elapsed();
    // old text, then every node replaced by the new text's
    let (mut eng, op) = item_engine();
    let u = eng.new_unit();
    let mut ids = Vec::new();
    for (kind, k) in &old {
        let a = item_args(eng.values(), kind, *k);
        ids.push(eng.push(u, op, a));
    }
    eng.evaluate_sequential().expect("old");
    for (id, (kind, k)) in ids.iter().zip(&new) {
        let a = item_args(eng.values(), kind, *k);
        eng.replace(*id, op, a);
    }
    let t0 = std::time::Instant::now();
    let re = eng.evaluate_sequential().expect("edit");
    let te = t0.elapsed();
    eprintln!("cold {tc:?}: {rc:?}");
    eprintln!("edit {te:?}: {re:?}");
    assert!(
        te.as_secs_f64() <= 1.5 * tc.as_secs_f64(),
        "edit {te:?} against cold {tc:?}"
    );
}

/// The same, the edit growing the text from 200 nodes to 1000 (the new
/// nodes inserted after the last old one).
#[test]
#[ignore = "engine gap: after the edit, reading the contributions so far costs 2.7x (1.49 s against 0.56 s cold)"]
fn growing_the_text_costs_a_cold_evaluation() {
    let n = 1000;
    let old = item_text(200, 0);
    let new = item_text(n, 7);
    let (mut cold, op) = item_engine();
    let u = cold.new_unit();
    for (kind, k) in &new {
        let a = item_args(cold.values(), kind, *k);
        cold.push(u, op, a);
    }
    let t0 = std::time::Instant::now();
    let rc = cold.evaluate_sequential().expect("cold");
    let tc = t0.elapsed();
    let (mut eng, op) = item_engine();
    let u = eng.new_unit();
    let mut ids = Vec::new();
    for (kind, k) in &old {
        let a = item_args(eng.values(), kind, *k);
        ids.push(eng.push(u, op, a));
    }
    eng.evaluate_sequential().expect("old");
    for (id, (kind, k)) in ids.iter().zip(&new) {
        let a = item_args(eng.values(), kind, *k);
        eng.replace(*id, op, a);
    }
    let mut last = *ids.last().unwrap();
    for (kind, k) in &new[ids.len()..] {
        let a = item_args(eng.values(), kind, *k);
        last = eng.insert_after(last, op, a);
    }
    let t0 = std::time::Instant::now();
    let re = eng.evaluate_sequential().expect("edit");
    let te = t0.elapsed();
    eprintln!("cold {tc:?}: {rc:?}");
    eprintln!("edit {te:?}: {re:?}");
    assert!(
        te.as_secs_f64() <= 1.5 * tc.as_secs_f64(),
        "edit {te:?} against cold {tc:?}"
    );
}

/// `["b", k]` defines `x`; `["a"]` reads `x` and leaves `["w", x]` as its
/// continuation (`Ctx::follow`), which leaves `["w", n-1]` in turn.
/// Changing `b` re-runs `a`, whose continuations are made again.
struct Cont;

static CONT: std::sync::OnceLock<OpId> = std::sync::OnceLock::new();

impl Operation for Cont {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "b" {
            vec![(v.field("x"), Locality::Local)]
        } else {
            Vec::new()
        })
    }
    fn claim(
        &self,
        a: &[ValueId],
        v: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        if tag(v, a) == "k" {
            Claim::Pending
        } else {
            Claim::None
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = CONT.get().copied().unwrap();
        let int = |i: usize| match a.get(i).map(|&x| v.get(x)).as_deref() {
            Some(Value::Int(n)) => *n,
            _ => 0,
        };
        match tag(v, &a).as_str() {
            "b" => cx.define(v.field("x"), v.int(int(1))),
            "a" => {
                let n = cx
                    .use_field(v.field("x"))?
                    .and_then(|y| v.as_int(y))
                    .unwrap_or(0);
                cx.child(op, vec![v.str("w"), v.int(1)]);
                cx.follow(op, vec![v.str("w"), v.int(n)]);
            }
            "k" => {
                // claims as many following siblings as `x` says, and
                // leaves their text as its continuations
                let n = cx
                    .use_field(v.field("x"))?
                    .and_then(|y| v.as_int(y))
                    .unwrap_or(0);
                let n = usize::try_from(n).unwrap_or(0);
                let mut got = 0;
                while got < n && cx.following(got).is_some() {
                    got += 1;
                }
                for i in 0..got {
                    cx.follow(op, vec![v.str("w"), v.int(i64::try_from(i).unwrap_or(0))]);
                }
                cx.claim(got);
            }
            "e" => {
                let n = cx
                    .use_env(v.field("e"))?
                    .and_then(|y| v.as_int(y))
                    .unwrap_or(0);
                cx.child(op, vec![v.str("w"), v.int(1)]);
                cx.follow(op, vec![v.str("w"), v.int(n)]);
            }
            "w" if int(1) > 0 => {
                cx.follow(op, vec![v.str("w"), v.int(int(1) - 1)]);
            }
            _ => {}
        }
        Ok(ValueId::UNIT)
    }
}

#[test]
fn continuations_made_again_after_a_change() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Cont));
    let _ = CONT.set(op);
    let u = eng.new_unit();
    let v = eng.values();
    let (b, a, z) = (
        vec![v.str("b"), v.int(3)],
        vec![v.str("a")],
        vec![v.str("w"), v.int(0)],
    );
    let nb = eng.push(u, op, b);
    eng.push(u, op, a);
    eng.push(u, op, z);
    eng.evaluate_sequential().expect("cold");
    let b2 = vec![eng.values().str("b"), eng.values().int(5)];
    eng.replace(nb, op, b2);
    eng.evaluate_sequential().expect("after the change");
    assert!(
        eng.check_invariants().is_empty(),
        "{:?}",
        eng.check_invariants()
    );
}

/// A node waiting for an environment field leaves its continuations once
/// the field is supplied.
#[test]
fn continuations_after_an_environment_wait() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Cont));
    let _ = CONT.set(op);
    let u = eng.new_unit();
    let v = eng.values();
    let (e, z) = (vec![v.str("e")], vec![v.str("w"), v.int(0)]);
    eng.push(u, op, e.clone());
    eng.push(u, op, e);
    eng.push(u, op, z);
    eng.evaluate_sequential().expect("waiting");
    let f = eng.values().field("e");
    let three = eng.values().int(3);
    eng.set_environment([(f, three)]);
    eng.evaluate_sequential().expect("supplied");
    assert!(
        eng.check_invariants().is_empty(),
        "{:?}",
        eng.check_invariants()
    );
}

/// A node that claims as many following siblings as a field says and
/// leaves their text as continuations; the field changes, so the claim
/// shrinks and grows.
#[test]
fn continuations_of_a_claim_that_changes() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Cont));
    let _ = CONT.set(op);
    let u = eng.new_unit();
    let v = eng.values();
    let nb = eng.push(u, op, vec![v.str("b"), v.int(3)]);
    let v = eng.values();
    eng.push(u, op, vec![v.str("k")]);
    for i in 0..6 {
        let v = eng.values();
        let a = vec![v.str("w"), v.int(i)];
        eng.push(u, op, a);
    }
    eng.evaluate_sequential().expect("cold");
    for x in [1, 5, 0, 4, 2] {
        let v = eng.values();
        let b = vec![v.str("b"), v.int(x)];
        eng.replace(nb, op, b);
        eng.evaluate_sequential().expect("after the change");
        assert!(
            eng.check_invariants().is_empty(),
            "{:?}",
            eng.check_invariants()
        );
    }
}

/// `["o"]` (bound `s`, effect pending) opens a scope before itself;
/// `["a", x]` (bound `f`, effect statically None) defines `f = x` locally;
/// `["x"]` (bound `g`, effect pending, resolved None) reads `f`, then
/// defines `g` and unfolds the continuation `["c"]`; `["c"]` (bound `s`, effect
/// pending) closes the scope; `["b"]` (bound `h`, effect pending, read
/// hint `f`) reads `f` and defines `h` where `f` is undefined, `q`
/// (outside its bound) where it reads the scope-local value.
struct SameBatch;

static SB_OP: std::sync::OnceLock<OpId> = std::sync::OnceLock::new();

impl Operation for SameBatch {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> phitex_defuse::MayDefine {
        let p = match tag(v, a).as_str() {
            "a" => "f",
            "x" => "g",
            "b" => "h",
            _ => "s",
        };
        phitex_defuse::MayDefine::Prefixes(vec![p.to_owned()])
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        matches!(tag(v, a).as_str(), "b" | "x").then(|| vec![v.field("f")])
    }
    fn claim(
        &self,
        _: &[ValueId],
        _: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        Claim::Pending
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> phitex_defuse::Effect {
        match tag(v, a).as_str() {
            "a" => phitex_defuse::Effect::None,
            _ => phitex_defuse::Effect::Pending,
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        use phitex_defuse::{Brackets, Effect};
        let v = cx.values();
        let a = cx.args();
        cx.claim(0);
        match tag(v, &a).as_str() {
            "o" => cx.set_effect(Effect::Scope(Brackets {
                closes: 0,
                opens: 1,
                before: true,
                kind: None,
                kind_pending: false,
            })),
            "a" => cx.define_with(v.field("f"), a[1], Locality::Local),
            "x" => {
                let _ = cx.use_field(v.field("f"))?;
                cx.define_with(v.field("g"), v.int(1), Locality::Global);
                cx.follow(SB_OP.get().copied().unwrap(), vec![v.str("c")]);
                cx.set_effect(Effect::None);
            }
            "c" => cx.set_effect(Effect::Close),
            "b" => {
                cx.set_effect(Effect::None);
                let f = cx.use_field(v.field("f"))?;
                let d = if f.is_some() { "q" } else { "h" };
                cx.define_with(v.field(d), v.int(1), Locality::Local);
                return Ok(f.unwrap_or_else(|| v.str("none")));
            }
            _ => {}
        }
        Ok(ValueId::UNIT)
    }
}

/// Two nodes ready at once (both waited on the same definition): `x`
/// unfolds a continuation that closes the scope the definition is local
/// to; `b`, after `x`, reads the field. The continuation does not exist
/// until `x` runs, so only `x`'s own effect can order `b` after it: with
/// it pending, `b` waits and reads past the close. Declared statically
/// None instead (true of `x`'s own command), `b` may run in `x`'s batch
/// against the state before the continuation exists, read the
/// scope-local value and define outside its bound (an undeclared
/// definition). The TeX client's hints declare "no scope" only for a
/// node whose whole text, continuations included, opens and closes none.
#[test]
fn a_reader_after_a_node_whose_continuation_closes_its_scope_waits() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(SameBatch));
    let _ = SB_OP.set(op);
    let u = eng.new_unit();
    let v = eng.values();
    let (o, a, x, b, twelve) = (v.str("o"), v.str("a"), v.str("x"), v.str("b"), v.int(12));
    eng.push(u, op, vec![o]);
    eng.push(u, op, vec![a, twelve]);
    eng.push(u, op, vec![x]);
    let b = eng.push(u, op, vec![b]);
    eng.add_root(b);
    let r = eng.evaluate_sequential();
    let got = eng.output(b).map(|x| eng.values().render(x));
    r.expect("evaluate");
    assert_eq!(got.as_deref(), Some("\"none\""));
    assert!(
        eng.check_invariants().is_empty(),
        "{:?}",
        eng.check_invariants()
    );
}

/// `["a"]` reads the environment field `mode`: at "cut" it claims its
/// first following sibling and places a continuation `["r"]` after
/// itself; at "whole" it claims every following `["s", x]` itself and
/// places nothing. `["r"]` claims every following `["s", x]`. `["s", x]`
/// counts its runs (it must never run: one of the others always claims
/// it).
struct ToyCut {
    s_runs: std::sync::atomic::AtomicU64,
}

static CUT_OP: std::sync::OnceLock<OpId> = std::sync::OnceLock::new();

impl Operation for ToyCut {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn claim(
        &self,
        a: &[ValueId],
        v: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        if matches!(tag(v, a).as_str(), "a" | "r") {
            Claim::Pending
        } else {
            Claim::None
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let ss = |cx: &Ctx<'_>| {
            let mut n = 0;
            while cx.following(n).is_some_and(|(_, xs)| tag(v, &xs) == "s") {
                n += 1;
            }
            n
        };
        match tag(v, &a).as_str() {
            "a" => {
                let m = cx.use_env(v.field("mode"))?;
                if m.is_some_and(|m| matches!(&*v.get(m), Value::Str(s) if &**s == "cut")) {
                    let _ = cx.following(0);
                    cx.claim(1);
                    let op = CUT_OP.get().copied().unwrap();
                    cx.follow(op, vec![v.str("r")]);
                } else {
                    let n = ss(cx);
                    cx.claim(n);
                }
                Ok(ValueId::UNIT)
            }
            "r" => {
                let n = ss(cx);
                cx.claim(n);
                Ok(ValueId::UNIT)
            }
            "s" => {
                self.s_runs.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(a.get(1).copied().unwrap_or(ValueId::UNIT))
            }
            _ => Ok(ValueId::UNIT),
        }
    }
}

/// The TeX client's run cut at a page break: the run claims the first
/// sibling and its continuation the rest; an edit before them moves the
/// break away, and the run, running again, claims all of them itself and
/// places no continuation. Its new claim reaches siblings that the
/// continuation it no longer places had claimed: they must stay claimed
/// (by the run) and never run. Seen: they are released when the old
/// continuation goes and run on their own (`apply_claim` skips a sibling
/// inert under another claimer before `set_unfolding` removes that
/// claimer).
#[test]
fn a_claim_that_takes_over_its_old_continuations_siblings() {
    let toy = Arc::new(ToyCut { s_runs: std::sync::atomic::AtomicU64::new(0) });
    let mut eng = Engine::new();
    let op = eng.register(toy.clone());
    let _ = CUT_OP.set(op);
    let u = eng.new_unit();
    let a = eng.push(u, op, vec![eng.values().str("a")]);
    for k in 0..4 {
        let (s, x) = (eng.values().str("s"), eng.values().int(k));
        eng.push(u, op, vec![s, x]);
    }
    let z = eng.push(u, op, vec![eng.values().str("z")]);
    eng.add_root(z);
    let f = eng.values().field("mode");
    let cut = eng.values().str("cut");
    eng.set_environment(vec![(f, cut)]);
    eng.evaluate_sequential().expect("evaluate");
    let r = std::sync::atomic::Ordering::Relaxed;
    assert_eq!(toy.s_runs.load(r), 0, "a sibling ran with mode cut");
    let whole = eng.values().str("whole");
    eng.set_environment(vec![(f, whole)]);
    eng.evaluate_sequential().expect("evaluate");
    assert_eq!(
        toy.s_runs.load(r),
        0,
        "a sibling the run now claims ran on its own after its old continuation went"
    );
    let _ = a;
}

/// `["d"]` defines `f` from the environment field `x` (its run's value
/// `x / 2`), declares it may define `f` only (a pending set), and
/// predicts `f` = 100 (`predicted_defs`, asked once per text); `["r"]`
/// reads `f` and the environment field `y`, outputs `f`; `["s"]`
/// defines `x` from the environment field `z`.
struct Pred;

impl Operation for Pred {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "d" | "s" => None,
            _ => Some(Vec::new()),
        }
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> phitex_defuse::MayDefine {
        match tag(v, a).as_str() {
            "d" => phitex_defuse::MayDefine::Names(vec![v.field("f")]),
            "s" => phitex_defuse::MayDefine::Names(vec![v.field("x")]),
            _ => phitex_defuse::MayDefine::Nothing,
        }
    }
    fn predicted_defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        (tag(v, a) == "d").then(|| vec![(v.field("f"), v.int(100))])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let int = |x: Option<ValueId>| x.and_then(|x| v.as_int(x)).unwrap_or(0);
        match tag(v, &a).as_str() {
            "s" => {
                let z = int(cx.use_field(v.field("z"))?);
                cx.define(v.field("x"), v.int(z));
                Ok(ValueId::UNIT)
            }
            "d" => {
                // (some work: the reader can run meanwhile)
                let mut h = 0u64;
                for i in 0..200_000u64 {
                    h = h.wrapping_mul(31).wrapping_add(i);
                }
                let x = int(cx.use_field(v.field("x"))?);
                // (a multiple of 3: no definition, the value before it
                // stands)
                if x % 3 != 0 {
                    cx.define(v.field("f"), v.int(x / 2 + i64::from(h == 7)));
                }
                Ok(ValueId::UNIT)
            }
            _ => {
                let _ = cx.use_field(v.field("y"))?;
                let f = int(cx.use_field(v.field("f"))?);
                Ok(v.int(f))
            }
        }
    }
}

/// A prediction changes speed, never results: a reader that ran on a
/// definer's predicted value (100) outputs the value the definer
/// defines, cold, and after edits where the definer runs again and
/// commits a value equal to its previous commit while the reader runs
/// again too (the definer's node and text the same across the
/// evaluations, its prediction asked once). Every scheduler, many seeds.
#[test]
fn a_reader_on_a_prediction_ends_on_the_value_its_definer_commits() {
    let schedulers: Vec<Box<dyn Fn(&mut Engine) -> phitex_defuse::Report>> = vec![
        Box::new(|e| e.evaluate_sequential().expect("evaluate")),
        Box::new(|e| {
            e.evaluate(
                &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 256),
                &mut phitex_defuse::Sequential,
            )
            .expect("evaluate")
        }),
    ];
    let mut bad = Vec::new();
    for (k, sched) in schedulers.iter().enumerate() {
        for seed in 0..40u64 {
            let mut eng = Engine::new();
            let op = eng.register(Arc::new(Pred));
            let u = eng.new_unit();
            let f = |e: &Engine, n: &str| e.values().field(n);
            let int = |e: &Engine, i: i64| e.values().int(i);
            let (fy, fz) = (f(&eng, "y"), f(&eng, "z"));
            let x = int(&eng, 10);
            eng.set_env(u, fz, x);
            let x = int(&eng, 0);
            eng.set_env(u, fy, x);
            let ff = f(&eng, "f");
            let x = int(&eng, 7);
            eng.set_env(u, ff, x);
            for t in ["s", "d"] {
                let a = vec![eng.values().str(t)];
                eng.push(u, op, a);
            }
            let a = vec![eng.values().str("r")];
            let r = eng.push(u, op, a);
            let random = |e: &mut Engine, s: u64| {
                e.evaluate(&mut phitex_defuse::RandomScheduler::new(s, true), &mut phitex_defuse::Sequential)
                    .expect("evaluate")
            };
            let run = |e: &mut Engine| if seed % 2 == 0 { sched(e) } else { random(e, seed) };
            run(&mut eng);
            let out = |e: &Engine| e.output(r).and_then(|x| e.values().as_int(x));
            if out(&eng) != Some(5) {
                bad.push(format!("sched {k} seed {seed} cold: {:?}", out(&eng)));
            }
            // the definer runs again (x 10 -> 11) and commits 5 again; the
            // reader runs again (y changed) while it is in work
            let x = int(&eng, 11);
            eng.set_env(u, fz, x);
            let x = int(&eng, 1);
            eng.set_env(u, fy, x);
            run(&mut eng);
            if out(&eng) != Some(5) {
                bad.push(format!("sched {k} seed {seed} same value: {:?}", out(&eng)));
            }
            // and a different value
            let x = int(&eng, 40);
            eng.set_env(u, fz, x);
            let x = int(&eng, 2);
            eng.set_env(u, fy, x);
            run(&mut eng);
            if out(&eng) != Some(20) {
                bad.push(format!("sched {k} seed {seed} new value: {:?}", out(&eng)));
            }
            // predicted and not defined: the value before the definer
            // (the environment's 7) reaches the reader
            let x = int(&eng, 9);
            eng.set_env(u, fz, x);
            let x = int(&eng, 3);
            eng.set_env(u, fy, x);
            run(&mut eng);
            if out(&eng) != Some(7) {
                bad.push(format!("sched {k} seed {seed} predicted, not defined: {:?}", out(&eng)));
            }
        }
    }
    assert!(bad.is_empty(), "{} wrong outputs: {:?}", bad.len(), &bad[..bad.len().min(8)]);
}

/// The prediction [`PredMade`]'s definer gives (a client's running
/// memo: it changes between evaluations).
static PRED_NOW: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(100);

/// As [`Pred`], the definer made by a parent: `["p"]` reads the
/// environment field `w` and places `["d"]` after it (`Ctx::follow`,
/// the same text every run: the same node); `["d"]` predicts `f` =
/// [`PRED_NOW`] as it stands when its text is analysed.
struct PredMade(std::sync::OnceLock<OpId>);

impl Operation for PredMade {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "d" | "s" => None,
            _ => Some(Vec::new()),
        }
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> phitex_defuse::MayDefine {
        match tag(v, a).as_str() {
            "d" => phitex_defuse::MayDefine::Names(vec![v.field("f")]),
            "s" => phitex_defuse::MayDefine::Names(vec![v.field("x")]),
            _ => phitex_defuse::MayDefine::Nothing,
        }
    }
    fn predicted_defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        (tag(v, a) == "d").then(|| vec![(v.field("f"), v.int(PRED_NOW.load(std::sync::atomic::Ordering::Relaxed)))])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let int = |x: Option<ValueId>| x.and_then(|x| v.as_int(x)).unwrap_or(0);
        match tag(v, &a).as_str() {
            "s" => {
                let z = int(cx.use_field(v.field("z"))?);
                cx.define(v.field("x"), v.int(z));
                Ok(ValueId::UNIT)
            }
            "p" => {
                let _ = cx.use_field(v.field("w"))?;
                cx.follow(*self.0.get().expect("op"), vec![v.str("d")]);
                Ok(ValueId::UNIT)
            }
            "d" => {
                let mut h = 0u64;
                for i in 0..200_000u64 {
                    h = h.wrapping_mul(31).wrapping_add(i);
                }
                let x = int(cx.use_field(v.field("x"))?);
                cx.define(v.field("f"), v.int(x / 2 + i64::from(h == 7)));
                Ok(ValueId::UNIT)
            }
            _ => {
                let _ = cx.use_field(v.field("y"))?;
                let f = int(cx.use_field(v.field("f"))?);
                Ok(v.int(f))
            }
        }
    }
}

/// A prediction changes speed, never results, also when the definer's
/// text is analysed again (its parent runs again and makes the same
/// continuation) after a reader ran on its earlier prediction, and the
/// new prediction equals what the definer then commits: the reader that
/// took the earlier one runs again. (PhiTeX: a page's finisher is a
/// positional node its scan step makes again; its predictions come from
/// the finishers run so far, and a later finisher read a box register's
/// predicted value from before the routine changed it: amsldoc shipped
/// a two-column page's stored column twice.)
#[test]
fn a_reader_on_an_earlier_prediction_runs_again_when_the_text_is_analysed_again() {
    use std::sync::atomic::Ordering::Relaxed;
    let mut bad = Vec::new();
    for seed in 0..60u64 {
        PRED_NOW.store(100, Relaxed);
        let mut eng = Engine::new();
        let ops = Arc::new(PredMade(std::sync::OnceLock::new()));
        let op = eng.register(ops.clone());
        let _ = ops.0.set(op);
        let u = eng.new_unit();
        let f = |e: &Engine, n: &str| e.values().field(n);
        let int = |e: &Engine, i: i64| e.values().int(i);
        let env = |e: &mut Engine, n: &str, i: i64| {
            let (fl, x) = (f(e, n), int(e, i));
            e.set_env(u, fl, x);
        };
        env(&mut eng, "z", 10);
        env(&mut eng, "y", 0);
        env(&mut eng, "w", 0);
        for t in ["s", "p"] {
            let a = vec![eng.values().str(t)];
            eng.push(u, op, a);
        }
        let a = vec![eng.values().str("r")];
        let r = eng.push(u, op, a);
        let run = |e: &mut Engine| {
            if seed % 3 == 0 {
                e.evaluate(
                    &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 256),
                    &mut phitex_defuse::Sequential,
                )
            } else {
                e.evaluate(&mut phitex_defuse::RandomScheduler::new(seed, true), &mut phitex_defuse::Sequential)
            }
            .expect("evaluate")
        };
        let out = |e: &Engine| e.output(r).and_then(|x| e.values().as_int(x));
        run(&mut eng);
        if out(&eng) != Some(5) {
            bad.push(format!("seed {seed} cold: {:?}", out(&eng)));
        }
        // the definer's value moves (x 10 -> 20, f 5 -> 10); the reader, the
        // parent and the definer all run again; the memo now predicts 10,
        // the value the definer commits
        PRED_NOW.store(10, Relaxed);
        env(&mut eng, "z", 20);
        env(&mut eng, "y", 1);
        env(&mut eng, "w", 1);
        run(&mut eng);
        if out(&eng) != Some(10) {
            bad.push(format!("seed {seed} moved: {:?}", out(&eng)));
        }
        // the same value again (x 21: f 10), the memo predicting 3 first
        // and 10 once the parent runs again
        PRED_NOW.store(3, Relaxed);
        env(&mut eng, "z", 21);
        env(&mut eng, "y", 2);
        run(&mut eng);
        PRED_NOW.store(10, Relaxed);
        env(&mut eng, "w", 2);
        env(&mut eng, "y", 3);
        run(&mut eng);
        if out(&eng) != Some(10) {
            bad.push(format!("seed {seed} same value: {:?}", out(&eng)));
        }
    }
    assert!(bad.is_empty(), "{} wrong outputs: {:?}", bad.len(), &bad[..bad.len().min(8)]);
}
