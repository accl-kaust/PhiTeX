//! Engine behaviour the client needs, on a toy client: each test is
//! self-contained, to forward as an engine report.

#![allow(clippy::many_single_char_names)]

use std::sync::Arc;

use defuse::{
    Claim, Ctx, Engine, EvalError, Field, Locality, MayDefine, NodeId, OpId, Operation, Stop, Value, ValueId, Values,
};

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
/// splices the child inline after the parent): the client needs this
/// for an unfolded body whose last node takes its argument from the text
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
        if tag(v, a) == "d" { None } else { Some(Vec::new()) }
    }
    fn claim(&self, a: &[ValueId], v: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if matches!(tag(v, a).as_str(), "r" | "f") {
            Claim::None
        } else {
            Claim::Pending
        }
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> defuse::Effect {
        if matches!(tag(v, a).as_str(), "r" | "f") {
            defuse::Effect::None
        } else {
            defuse::Effect::Pending
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "c" => {
                cx.claim(1);
                cx.set_effect(defuse::Effect::None);
                Ok(ValueId::UNIT)
            }
            "d" => {
                cx.claim(0);
                cx.set_effect(defuse::Effect::None);
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
    eng.declare_accumulator(acc, Arc::new(defuse::SeqMonoid));
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
    fn claim(&self, a: &[ValueId], v: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if tag(v, a) == "c" { Claim::Pending } else { Claim::None }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let acc = v.field("acc");
        match tag(v, &a).as_str() {
            "c" => {
                let first = cx.following(0).and_then(|(_, xs)| xs.get(1).copied());
                let n = if first.is_some_and(|x| matches!(&*v.get(x), Value::Str(s) if &**s == "x")) {
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

fn toy3_unit(eng: &mut Engine, op: OpId, x: &str) -> (Vec<defuse::NodeId>, defuse::NodeId) {
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
    eng.declare_accumulator(acc, Arc::new(defuse::SeqMonoid));
    let (ns, f) = toy3_unit(&mut eng, op, "x");
    eng.evaluate_sequential().expect("evaluate");
    let a = vec![eng.values().str("s"), eng.values().str("x2")];
    eng.replace(ns[1], op, a);
    eng.evaluate_sequential().expect("evaluate after the edit");
    let got = eng.output(f).map(|x| eng.values().render(x));

    let mut cold = Engine::new();
    let op2 = cold.register(Arc::new(Toy3));
    let acc2 = cold.values().field("acc");
    cold.declare_accumulator(acc2, Arc::new(defuse::SeqMonoid));
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
    fn claim(&self, a: &[ValueId], v: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if tag(v, a) == "c" { Claim::Pending } else { Claim::None }
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
                Ok(f.map_or_else(|| v.str("none"), |(_, xs)| xs.get(1).copied().unwrap_or(ValueId::UNIT)))
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

fn toy4_unit(eng: &mut Engine, op: OpId, t: &str) -> (defuse::NodeId, defuse::NodeId) {
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
    eng.declare_accumulator(acc, Arc::new(defuse::SeqMonoid));
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
    cold.declare_accumulator(acc2, Arc::new(defuse::SeqMonoid));
    let (p2, f2) = toy4_unit(&mut cold, op2, "new");
    cold.evaluate_sequential().expect("cold evaluate");
    let c2 = cold.children(p2)[0];
    let want = (
        cold.output(c2).map(|x| cold.values().render(x)),
        cold.output(f2).map(|x| cold.values().render(x)),
    );
    assert_eq!(got, want, "(claimer's output, acc) after the edit differ from cold");
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
    fn claim(&self, _: &[ValueId], _: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        Claim::None
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> defuse::Effect {
        use defuse::{Brackets, Effect};
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
        if tag(v, a) == "i" { None } else { Some(Vec::new()) }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) == "i" {
            // An environment read: the node waits while the client has
            // neither supplied the field nor declared it absent.
            if let Some(x) = cx.use_env(v.field("file"))? {
                cx.define(v.field("x"), x);
            }
            return Ok(ValueId::UNIT);
        }
        Ok(cx.use_field(v.field("x"))?.unwrap_or_else(|| v.str("none")))
    }
}

/// An unfolding node whose environment input the client has not supplied yet:
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
    assert_eq!(second.as_deref(), Some("5"), "r did not re-run after i defined x");
    assert_eq!(
        first, None,
        "r ran ahead of i on the first evaluation (saw {first:?}) while i's file was undefined"
    );
}

// ---- an environment wait before an accumulator reader (client report)

/// `["i"]`: pending definition set; reads env field `file` with `use_env`
/// and, if supplied, defines `x`.  `["a"]`: reads the fold of the
/// accumulator `acc` over the unit.
struct EnvAccOp;

impl Operation for EnvAccOp {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        if tag(v, a) == "i" { None } else { Some(Vec::new()) }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "i" => {
                if let Some(x) = cx.use_env(v.field("file"))? {
                    cx.define(v.field("x"), x);
                }
                Ok(ValueId::UNIT)
            }
            "a" => cx.acc_range(v.field("acc"), None),
            _ => Ok(cx.use_field(v.field("x"))?.unwrap_or_else(|| v.str("none"))),
        }
    }
}

#[test]
fn an_env_wait_before_an_accumulator_reader_is_not_a_cycle() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(EnvAccOp));
    let acc = eng.values().field("acc");
    eng.declare_accumulator(acc, Arc::new(defuse::SeqMonoid));
    let u = eng.new_unit();
    let i = eng.push(u, op, vec![eng.values().str("i")]);
    let a = eng.push(u, op, vec![eng.values().str("a")]);
    eng.add_root(a);
    let rep = eng.evaluate_sequential();
    assert!(rep.is_ok(), "an env wait ended as {:?}", rep.err());
    let rep = rep.unwrap();
    assert_eq!(
        rep.env_stalled, 2,
        "i waits for the field, a waits on the range holding i"
    );
    assert_eq!(rep.env_waits, vec![(i, eng.values().field("file"))]);
}

/// Waits for an environment field whose name it makes as it runs (no
/// one else holds the name's value); interns garbage first so the sweep
/// has work.
struct FreshName;

impl Operation for FreshName {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        for k in 0..20_000 {
            let _ = v.str(&format!("garbage-{k}"));
        }
        let name = format!("fresh-{}", 40 + 2);
        cx.use_env(v.field(&name))?;
        Ok(ValueId::UNIT)
    }
}

#[test]
fn env_wait_field_names_survive_the_evaluation() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(FreshName));
    let u = eng.new_unit();
    let _first = eng.push(u, eng.open_op(), Vec::new());
    eng.evaluate_sequential().expect("evaluate");
    let w = eng.push(u, op, Vec::new());
    eng.add_root(w);
    let rep = eng.evaluate_sequential().expect("evaluate");
    let (_, f) = rep.env_waits.first().copied().expect("a wait");
    // panics today: "unknown or freed value"
    let name = eng.values().render(f.0);
    assert_eq!(name, "\"fresh-42\"");
}

/// `["w", n]`: no claim, empty defs; when n > 0 it continues with ["w", n-1]
/// as a continuation at its own depth.
struct Cont;

impl Operation for Cont {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let n = match &*v.get(a[1]) {
            Value::Int(n) => *n,
            _ => 0,
        };
        if n > 0 {
            let op = cx.op();
            cx.follow(op, vec![v.str("w"), v.int(n - 1)]);
        }
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_continuation_stays_at_its_nodes_depth() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Cont));
    let u = eng.new_unit();
    let w = eng.push(u, op, vec![eng.values().str("w"), eng.values().int(5000)]);
    eng.add_root(w);
    // With children instead of continuations: RunawayCreation at the depth limit.
    eng.evaluate_sequential().expect("a continuation chain is not nesting");
    assert_eq!(eng.live_nodes(), 5001);
    // Every continuation is a top-level node (no parent), owned by its creator.
    let info = eng.node_info(w).expect("root");
    assert_eq!(info.parent, None);
    let mut cur = w;
    for _ in 0..5000 {
        let owned = eng.children(cur);
        assert_eq!(owned.len(), 1, "one continuation, owned by its creator");
        assert_eq!(
            eng.node_info(owned[0]).expect("live").parent,
            None,
            "at the creator's depth"
        );
        cur = owned[0];
    }
    assert_eq!(eng.children(cur), Vec::new(), "the last continuation unfolds nothing");
}

/// `["b", k]` defines `x`; `["a"]` reads `x` and leaves `["w", x]` as its
/// continuation (`Ctx::follow`), which leaves `["w", n-1]` in turn.
/// Changing `b` re-runs `a`, whose continuations are made again.
struct ClaimCont;

static CLAIM_CONT: std::sync::OnceLock<OpId> = std::sync::OnceLock::new();

impl Operation for ClaimCont {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "b" {
            vec![(v.field("x"), Locality::Local)]
        } else {
            Vec::new()
        })
    }
    fn claim(&self, a: &[ValueId], v: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if tag(v, a) == "k" { Claim::Pending } else { Claim::None }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = CLAIM_CONT.get().copied().unwrap();
        let int = |i: usize| match a.get(i).map(|&x| v.get(x)).as_deref() {
            Some(Value::Int(n)) => *n,
            _ => 0,
        };
        match tag(v, &a).as_str() {
            "b" => cx.define(v.field("x"), v.int(int(1))),
            "a" => {
                let n = cx.use_field(v.field("x"))?.and_then(|y| v.as_int(y)).unwrap_or(0);
                cx.child(op, vec![v.str("w"), v.int(1)]);
                cx.follow(op, vec![v.str("w"), v.int(n)]);
            }
            "k" => {
                // claims as many following siblings as `x` says, and
                // leaves their text as its continuations
                let n = cx.use_field(v.field("x"))?.and_then(|y| v.as_int(y)).unwrap_or(0);
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
                let n = cx.use_env(v.field("e"))?.and_then(|y| v.as_int(y)).unwrap_or(0);
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
    let op = eng.register(Arc::new(ClaimCont));
    let _ = CLAIM_CONT.set(op);
    let u = eng.new_unit();
    let v = eng.values();
    let (b, a, z) = (vec![v.str("b"), v.int(3)], vec![v.str("a")], vec![v.str("w"), v.int(0)]);
    let nb = eng.push(u, op, b);
    eng.push(u, op, a);
    eng.push(u, op, z);
    eng.evaluate_sequential().expect("cold");
    let b2 = vec![eng.values().str("b"), eng.values().int(5)];
    eng.replace(nb, op, b2);
    eng.evaluate_sequential().expect("after the change");
    assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
}

/// A node waiting for an environment field leaves its continuations once
/// the field is supplied.
#[test]
fn continuations_after_an_environment_wait() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(ClaimCont));
    let _ = CLAIM_CONT.set(op);
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
    assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
}

/// A node that claims as many following siblings as a field says and
/// leaves their text as continuations; the field changes, so the claim
/// shrinks and grows.
#[test]
fn continuations_of_a_claim_that_changes() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(ClaimCont));
    let _ = CLAIM_CONT.set(op);
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
        assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
    }
}

/// `["ap", k]` contributes `k` to `acc` and reads nothing; `["o"]` opens a
/// scope; `["rd"]` reads the contributions since the nearest `o` before it,
/// anchored at that opener's id (a node the run does not hold).
struct ListOps;

impl Operation for ListOps {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> defuse::Effect {
        if tag(v, a) == "o" {
            defuse::Effect::Open
        } else {
            defuse::Effect::None
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "ap" => {
                let k = a[1];
                cx.contribute(v.field("acc"), v.seq_unit(k));
                Ok(ValueId::UNIT)
            }
            "rd" => {
                let opener = cx.prev_sibling_where(&|_, a| tag(v, a) == "o");
                cx.acc_range(v.field("acc"), opener)
            }
            _ => Ok(ValueId::UNIT),
        }
    }
}

#[test]
fn a_range_read_anchored_at_an_opener_the_run_does_not_hold() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(ListOps));
    let acc = eng.values().field("acc");
    eng.declare_accumulator(acc, Arc::new(defuse::SeqMonoid));
    let u = eng.new_unit();
    let mk = |eng: &mut Engine, t: &str, k: i64| {
        let v = eng.values();
        let a = if t == "ap" {
            vec![v.str(t), v.int(k)]
        } else {
            vec![v.str(t)]
        };
        eng.push(u, op, a)
    };
    mk(&mut eng, "ap", 100);
    mk(&mut eng, "o", 0);
    let a1 = mk(&mut eng, "ap", 1);
    mk(&mut eng, "ap", 2);
    mk(&mut eng, "ap", 3);
    let rd = mk(&mut eng, "rd", 0);
    eng.evaluate_sequential().expect("evaluate");
    let items = |eng: &Engine| -> Vec<i64> {
        let v = eng.values();
        v.seq_to_vec(eng.output(rd).expect("rd output"))
            .into_iter()
            .filter_map(|x| v.as_int(x))
            .collect()
    };
    assert_eq!(items(&eng), vec![1, 2, 3], "from the opener, not the unit start");
    // Appends read nothing, so they do not chain: the reader waits for them
    // all at once.
    assert!(eng.critical_path() <= 2, "critical path {}", eng.critical_path());
    let v = eng.values();
    let a = vec![v.str("ap"), v.int(9)];
    eng.replace(a1, op, a);
    eng.evaluate_sequential().expect("after the edit");
    assert_eq!(items(&eng), vec![9, 2, 3]);
}

/// `["rec", k]` contributes `k` and outputs its own id as a number; `["rd",
/// raw]` reads the contributions after the node whose id is `raw`.
struct RecOps;

impl Operation for RecOps {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "rec" => {
                cx.contribute(v.field("acc"), v.seq_unit(a[1]));
                // The id's 64 bits, as the toy's integer.
                Ok(v.int(i64::from_ne_bytes(cx.node().raw().to_ne_bytes())))
            }
            "rd" => {
                let from = v
                    .as_int(a[1])
                    .map(|r| NodeId::from_raw(u64::from_ne_bytes(r.to_ne_bytes())));
                cx.acc_range(v.field("acc"), from)
            }
            _ => Ok(ValueId::UNIT),
        }
    }
}

#[test]
fn a_recorded_node_id_anchors_a_later_range_read() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(RecOps));
    let acc = eng.values().field("acc");
    eng.declare_accumulator(acc, Arc::new(defuse::SeqMonoid));
    let u = eng.new_unit();
    let push = |eng: &mut Engine, t: &str, k: i64| {
        let v = eng.values();
        eng.push(u, op, vec![v.str(t), v.int(k)])
    };
    push(&mut eng, "rec", 1);
    let a = push(&mut eng, "rec", 2);
    let b = push(&mut eng, "rec", 3);
    push(&mut eng, "rec", 4);
    // The reader anchors at `a` through the number `a` output.
    let raw = i64::from_ne_bytes(a.raw().to_ne_bytes());
    let c = push(&mut eng, "rd", raw);
    eng.evaluate_sequential().expect("evaluate");
    let items = |eng: &Engine| -> Vec<i64> {
        let v = eng.values();
        v.seq_to_vec(eng.output(c).expect("output"))
            .into_iter()
            .filter_map(|x| v.as_int(x))
            .collect()
    };
    assert_eq!(items(&eng), vec![3, 4], "exactly the contributions after a");
    assert_eq!(eng.output(a).and_then(|o| eng.values().as_int(o)), Some(raw));
    // Replacing `a` keeps its id and changes nothing after it: the reader
    // keeps its result without running.
    let v = eng.values();
    let na = vec![v.str("rec"), v.int(20)];
    eng.replace(a, op, na);
    let r = eng.evaluate_sequential().expect("after replacing a");
    assert_eq!(r.nodes_run, 1, "{r:?}");
    assert_eq!(items(&eng), vec![3, 4]);
    // A change inside the range runs the reader again.
    let v = eng.values();
    let nb = vec![v.str("rec"), v.int(30)];
    eng.replace(b, op, nb);
    let r = eng.evaluate_sequential().expect("after replacing b");
    assert!(r.nodes_run >= 2, "{r:?}");
    assert_eq!(items(&eng), vec![30, 4]);
}

/// `["c", i, n]`: node `i` of a chain reads `x<i-1>`, defines `x<i>` and
/// leaves node `i + 1` as its continuation; definitions pending until the
/// node runs.
struct Chain;

static CHAIN: std::sync::OnceLock<OpId> = std::sync::OnceLock::new();

impl Operation for Chain {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let int = |k: usize| match a.get(k).map(|&x| v.get(x)).as_deref() {
            Some(Value::Int(n)) => *n,
            _ => 0,
        };
        let (i, n) = (int(1), int(2));
        let prev = if i > 0 {
            cx.use_field(v.field(&format!("x{}", i - 1)))?
                .and_then(|y| v.as_int(y))
                .unwrap_or(0)
        } else {
            0
        };
        cx.define(v.field(&format!("x{i}")), v.int(prev + 1));
        if i + 1 < n {
            let op = CHAIN.get().copied().unwrap();
            cx.follow(op, vec![v.str("c"), v.int(i + 1), v.int(n)]);
        }
        Ok(ValueId::UNIT)
    }
}

/// A continuation starts above the run that made it, so a chain of
/// continuations each reading its predecessor runs each node once (no
/// raise), and the critical chain walks the whole chain back to the first
/// node: one link per node, the last one its creation by nothing.
#[test]
fn the_critical_chain_follows_continuations_to_the_first_node() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Chain));
    let _ = CHAIN.set(op);
    let u = eng.new_unit();
    let v = eng.values();
    let first = eng.push(u, op, vec![v.str("c"), v.int(0), v.int(100)]);
    let r = eng.evaluate_sequential().expect("chain");
    assert_eq!(r.runs, 100, "{r:?}");
    assert_eq!(r.raises, 0, "{r:?}");
    assert_eq!(r.critical_path, 99, "{r:?}");
    let chain = eng.critical_chain();
    assert_eq!(chain.len(), 100, "{chain:?}");
    let mut nodes: Vec<NodeId> = chain.iter().map(|c| c.0).collect();
    nodes.dedup();
    assert_eq!(nodes.len(), 100, "{chain:?}");
    assert!(
        chain[..99].iter().all(|c| matches!(c.1, defuse::Cause::Changed { .. })),
        "{chain:?}"
    );
    assert_eq!(chain[99].0, first);
    assert!(
        matches!(
            chain[99].1,
            defuse::Cause::Created {
                by: None,
                from_hint: None
            }
        ),
        "{:?}",
        chain[99]
    );
    assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
}

/// `["m", name]`: a node whose definitions are known only when it runs,
/// bounded by `may_define` to the name `a`; it defines `name`.
struct Bounded;

impl Operation for Bounded {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define(&self, _: &[ValueId], v: &Values) -> MayDefine {
        MayDefine::Names(vec![v.field("a")])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let name = match a.get(1).map(|&x| v.get(x)).as_deref() {
            Some(Value::Str(s)) => s.to_string(),
            _ => String::new(),
        };
        cx.define(v.field(&name), v.int(1));
        Ok(ValueId::UNIT)
    }
}

/// A dynamic definition outside the node's `may_define` answer is an
/// error (readers may have been bound past the node while it was pending),
/// as an undeclared static definition is.
#[test]
fn a_dynamic_definition_outside_may_define_is_an_error() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Bounded));
    let u = eng.new_unit();
    let v = eng.values();
    eng.push(u, op, vec![v.str("m"), v.str("a")]);
    eng.evaluate_sequential().expect("within the bound");
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Bounded));
    let u = eng.new_unit();
    let v = eng.values();
    let n = eng.push(u, op, vec![v.str("m"), v.str("b")]);
    let b = eng.values().field("b");
    match eng.evaluate_sequential() {
        Err(EvalError::UndeclaredDefinition { node, field }) => {
            assert_eq!(node, n);
            assert_eq!(field, b);
        }
        other => panic!("expected UndeclaredDefinition, got {other:?}"),
    }
}

/// `["n", k]`: outputs `k`.
struct Num;

impl Operation for Num {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let a = cx.args();
        Ok(a.get(1).copied().unwrap_or(ValueId::UNIT))
    }
}

/// Edits queued from another thread are absorbed by the evaluation, which
/// reports their version; the ids of the nodes they inserted come back
/// by version.
#[test]
fn an_edit_queue_feeds_an_evaluation_from_another_thread() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Num));
    let u = eng.new_unit();
    let v = eng.values();
    let first = eng.push(u, op, vec![v.str("n"), v.int(1)]);
    let queue = defuse::EditQueue::new();
    let q = queue.clone();
    let (two, three) = (eng.values().int(2), eng.values().int(3));
    let n = eng.values().str("n");
    std::thread::spawn(move || {
        q.push(
            defuse::Edit::Push {
                unit: u,
                op,
                args: vec![n, two],
            },
            1,
        );
        q.push(
            defuse::Edit::Push {
                unit: u,
                op,
                args: vec![n, three],
            },
            2,
        );
        q.push(
            defuse::Edit::Replace {
                node: first,
                op,
                args: vec![n, three],
            },
            3,
        );
    })
    .join()
    .expect("the queuing thread");
    assert_eq!(queue.pending(), 3);
    let mut queue = queue;
    let r = eng
        .evaluate_absorbing(&mut defuse::Greedy::default(), &mut defuse::Sequential, &mut queue)
        .expect("absorbing");
    assert_eq!(r.version, 3, "{r:?}");
    assert_eq!(r.edits_applied, vec![(3, 0)], "{r:?}");
    assert_eq!(queue.applied(), 3);
    assert_eq!(queue.pending(), 0);
    let made = queue.take_made();
    assert_eq!(made.len(), 2, "{made:?}");
    assert_eq!(made[0].0, 1);
    assert_eq!(made[1].0, 2);
    assert_eq!(eng.output(made[0].1), Some(two));
    assert_eq!(eng.output(made[1].1), Some(three));
    assert_eq!(eng.output(first), Some(three));
    assert_eq!(queue.take_made(), Vec::new());
}

/// `["t", k]`: claims `k` following siblings, decided when it runs.
struct Taker;

static TAKER_RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Operation for Taker {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn claim(&self, a: &[ValueId], v: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if tag(v, a) == "t" { Claim::Pending } else { Claim::None }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) == "t" {
            TAKER_RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let k = match a.get(1).map(|&x| v.get(x)).as_deref() {
                Some(Value::Int(n)) => usize::try_from(*n).unwrap_or(0),
                _ => 0,
            };
            let mut got = 0;
            while got < k && cx.following(got).is_some() {
                got += 1;
            }
            cx.claim(got);
        }
        Ok(ValueId::UNIT)
    }
}

/// A claim a node's own run makes does not make the node run again: the
/// run made it from the text it saw.
#[test]
fn a_claim_made_by_the_run_itself_does_not_rerun_the_claimer() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Taker));
    let u = eng.new_unit();
    let v = eng.values();
    let taker = eng.push(u, op, vec![v.str("t"), v.int(2)]);
    for i in 0..3 {
        let v = eng.values();
        let a = vec![v.str("w"), v.int(i)];
        eng.push(u, op, a);
    }
    TAKER_RUNS.store(0, std::sync::atomic::Ordering::Relaxed);
    eng.evaluate_sequential().expect("cold");
    assert_eq!(TAKER_RUNS.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(eng.children(taker).len(), 0);
    assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
    // A changed claim count (an edit) runs it again, once.
    let v = eng.values();
    let a = vec![v.str("t"), v.int(1)];
    eng.replace(taker, op, a);
    eng.evaluate_sequential().expect("after the edit");
    assert_eq!(TAKER_RUNS.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
}

/// `["d", k]`: outputs `k` and emits a diagnostic; `["r", k]` reads the
/// output of the `k`-th node of the unit (by its id passed as an arg).
struct Diag;

impl Operation for Diag {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let a = cx.args();
        let v = cx.values();
        let k = a.get(1).copied().unwrap_or(ValueId::UNIT);
        let msg = v.str("careful");
        cx.diagnostic(msg);
        Ok(k)
    }
}

/// A run's diagnostics are readable once the node is done, under every
/// policy, and a node that is not done has none.
#[test]
fn a_runs_diagnostics_are_kept_with_its_result() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Diag));
    let u = eng.new_unit();
    let d = eng.values().str("d");
    let ints: Vec<ValueId> = (0..40).map(|i| eng.values().int(i)).collect();
    let nodes: Vec<NodeId> = ints.iter().map(|&k| eng.push(u, op, vec![d, k])).collect();
    assert_eq!(eng.diagnostics(nodes[0]), &[]);
    eng.evaluate(
        &mut defuse::Greedy::speculative(defuse::Speculation::All, 64),
        &mut defuse::Sequential,
    )
    .expect("evaluate");
    let careful = eng.values().str("careful");
    for &n in &nodes {
        assert_eq!(eng.diagnostics(n), &[careful], "{n:?}");
    }
    let hundred = eng.values().int(100);
    eng.replace(nodes[3], op, vec![d, hundred]);
    eng.evaluate_sequential().expect("evaluate");
    assert_eq!(eng.diagnostics(nodes[3]), &[careful]);
    assert_eq!(eng.output(nodes[3]), Some(hundred));
    eng.remove(nodes[3]);
    assert_eq!(eng.diagnostics(nodes[3]), &[]);
}

/// `["open", k]` opens a scope after itself whose kind `k` it sets when it
/// runs; `["kind"]` outputs the kind of its scope; `["val", x]` outputs
/// `x`; `["read"]` outputs what `v` is bound to.
struct Kinds;

impl Operation for Kinds {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "val" => vec![(v.field("v"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> defuse::Effect {
        match tag(v, a).as_str() {
            "open" => defuse::Effect::Scope(defuse::Brackets {
                opens: 1,
                kind_pending: true,
                ..defuse::Brackets::default()
            }),
            _ => defuse::Effect::None,
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let a = cx.args();
        let v = cx.values();
        match tag(v, &a).as_str() {
            "open" => {
                let k = a[1];
                cx.set_kind(k);
                Ok(k)
            }
            "kind" => Ok(cx.scope_kind()?.unwrap_or(ValueId::UNIT)),
            "val" => {
                let x = a[1];
                cx.define(v.field("v"), x);
                Ok(x)
            }
            _ => Ok(cx.use_field(v.field("v"))?.unwrap_or(ValueId::UNIT)),
        }
    }
}

/// A static opener whose kind the run sets: readers of the kind wait for
/// the opener's run (a definition-set wait, not a scope-effect one), a
/// reader of anything else inside the scope waits for nothing, and the
/// kind follows the opener's text.
#[test]
fn a_static_opener_sets_its_kind_when_it_runs() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Kinds));
    let u = eng.new_unit();
    let v = eng.values();
    let (open, kind, val, read) = (v.str("open"), v.str("kind"), v.str("val"), v.str("read"));
    let (doc, list, one) = (v.str("doc"), v.str("list"), v.int(1));
    let _ = eng.push(u, op, vec![val, one]);
    let opener = eng.push(u, op, vec![open, doc]);
    let k = eng.push(u, op, vec![kind]);
    let r = eng.push(u, op, vec![read]);
    let rep = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(eng.output(k), Some(doc));
    assert_eq!(eng.output(r), Some(one));
    // The kind reader waited on the opener's run; nothing waited on a
    // scope effect (the brackets were known from the text).
    let kinds: Vec<&str> = rep.waits_by_kind.iter().map(|w| w.0).collect();
    assert!(!kinds.contains(&"scope effect"), "{:?}", rep.waits_by_kind);
    assert!(rep.top_waited.iter().any(|w| w.0 == opener), "{:?}", rep.top_waited);
    eng.replace(opener, op, vec![open, list]);
    let rep = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(eng.output(k), Some(list));
    assert!(rep.nodes_run <= 2, "{rep:?}");
}

/// `["sum"]` reads the environment fields a, b, c in turn and outputs
/// their sum; it parks its progress before each read and resumes from it,
/// counting the times it started from the top.
struct Sum;

static SUM_STARTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Operation for Sum {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let names = ["a", "b", "c"];
        let (mut i, mut acc) = if let Some(t) = cx.parked().and_then(|t| t.downcast::<(usize, i64)>().ok()) {
            *t
        } else {
            SUM_STARTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            (0, 0)
        };
        while i < names.len() {
            cx.park(Box::new((i, acc)));
            let x = cx.use_env(v.field(names[i]))?;
            acc += x.and_then(|x| v.as_int(x)).unwrap_or(0);
            i += 1;
        }
        Ok(v.int(acc))
    }
}

/// A run that stops at a read resumes there on its next attempt when
/// every read it made still holds; it starts over when one changed; the
/// report counts both.
#[test]
fn a_stopped_run_resumes_at_its_read_while_its_earlier_reads_hold() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Sum));
    let u = eng.new_unit();
    let sum = eng.values().str("sum");
    let n = eng.push(u, op, vec![sum]);
    let (a, b, c) = (
        eng.values().field("a"),
        eng.values().field("b"),
        eng.values().field("c"),
    );
    let (one, two, three, ten) = (
        eng.values().int(1),
        eng.values().int(2),
        eng.values().int(3),
        eng.values().int(10),
    );
    eng.set_environment([(a, one)]);
    let r = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(r.env_waits, vec![(n, b)]);
    assert_eq!(SUM_STARTS.load(std::sync::atomic::Ordering::Relaxed), 1);
    // b arrives: the run resumes after a (one start so far).
    eng.set_environment([(a, one), (b, two)]);
    let r = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(r.env_waits, vec![(n, c)]);
    assert_eq!((r.resumed, r.restarted), (1, 0), "{r:?}");
    assert_eq!(SUM_STARTS.load(std::sync::atomic::Ordering::Relaxed), 1);
    // c arrives but a changed: the kept state is dropped, the run starts
    // over and sees the new a.
    eng.set_environment([(a, ten), (b, two), (c, three)]);
    let r = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(r.env_waits, vec![]);
    assert_eq!((r.resumed, r.restarted), (0, 1), "{r:?}");
    assert_eq!(SUM_STARTS.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert_eq!(eng.output(n), Some(eng.values().int(15)));
    // A completed run keeps nothing: the next re-run starts from the top.
    eng.set_environment([(a, ten), (b, three), (c, three)]);
    let r = eng.evaluate_sequential().expect("evaluate");
    assert_eq!((r.resumed, r.restarted), (0, 0), "{r:?}");
    assert_eq!(SUM_STARTS.load(std::sync::atomic::Ordering::Relaxed), 3);
    assert_eq!(eng.output(n), Some(eng.values().int(16)));
}
