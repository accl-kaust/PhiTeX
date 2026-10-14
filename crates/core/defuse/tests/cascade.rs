//! A chain of nodes with dynamic definition sets, each reading field `x`
//! from its predecessor and defining `x` with the value it read: an
//! edit to the first node's text that changes nothing it defines or
//! outputs should run that node only (the others' inputs are equal).

use std::sync::Arc;

use phitex_defuse::{
    Claim, Ctx, Effect, Engine, Field, Locality, MayDefine, NodeId, OpId, Operation, Stop, ValueId,
    Values,
};

/// `[text, mode]`: reads `x` (none: 1), defines `x` with it, outputs
/// nothing. `mode` 0: dynamic, bounded to `x` (`may_define` names it);
/// 1: dynamic, unbounded; 2: static (`defs` lists `x`).
struct Chain;

fn mode(v: &Values, a: &[ValueId]) -> i64 {
    a.get(1).map_or(0, |&m| v.as_int(m).unwrap_or(0))
}

/// The extras of `a` (bits): 1 a pending scope effect (none when run),
/// 2 a pending claim (none when run), 4 a global definition, 8 reads
/// the scope kind.
fn extra(v: &Values, a: &[ValueId]) -> i64 {
    a.get(2).map_or(0, |&m| v.as_int(m).unwrap_or(0))
}

impl Operation for Chain {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        (mode(v, a) == 2).then(|| vec![(v.field("x"), Locality::Local)])
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> MayDefine {
        if mode(v, a) == 0 {
            MayDefine::Names(vec![v.field("x")])
        } else {
            MayDefine::Any
        }
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> Effect {
        if extra(v, a) & 1 != 0 {
            Effect::Pending
        } else {
            Effect::None
        }
    }
    fn claim(
        &self,
        a: &[ValueId],
        v: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        if extra(v, a) & 2 != 0 {
            Claim::Pending
        } else {
            Claim::None
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let e = extra(v, &a);
        if e & 8 != 0 {
            let _ = cx.scope_kind()?;
        }
        let x = cx.use_field(v.field("x"))?.unwrap_or_else(|| v.int(1));
        if e & 1 != 0 {
            cx.set_effect(Effect::None);
        }
        if e & 2 != 0 {
            cx.claim(0);
        }
        if e & 4 != 0 {
            cx.define_with(v.field("x"), x, Locality::Global);
        } else {
            cx.define(v.field("x"), x);
        }
        Ok(ValueId::UNIT)
    }
}

fn chain(mode: i64, extra: i64) -> (u64, u64) {
    let mut eng = Engine::new();
    let op: OpId = eng.register(Arc::new(Chain));
    let u = eng.new_unit();
    let nodes: Vec<NodeId> = (0..18)
        .map(|i| {
            let a = vec![
                eng.values().str(&format!("node {i}")),
                eng.values().int(mode),
                eng.values().int(extra),
            ];
            eng.push(u, op, a)
        })
        .collect();
    eng.evaluate_sequential().expect("cold");
    let a = vec![
        eng.values().str("node 0, edited"),
        eng.values().int(mode),
        eng.values().int(extra),
    ];
    eng.replace(nodes[0], op, a);
    let r = eng.evaluate_sequential().expect("edit");
    (r.nodes_run, r.runs)
}

#[test]
fn an_edit_that_changes_nothing_runs_one_node() {
    let mut out = Vec::new();
    for (m, name) in [
        (2, "static"),
        (0, "dynamic bounded"),
        (1, "dynamic unbounded"),
    ] {
        for e in 0..16 {
            if m == 2 && e & 4 != 0 {
                continue;
            }
            let (nodes, runs) = chain(m, e);
            eprintln!("cascade: {name}, extras {e:04b}: {nodes} nodes run, {runs} runs (of 18)");
            out.push((name, e, nodes));
        }
    }
    assert!(out.iter().all(|&(_, _, n)| n == 1), "{out:?}");
}

/// `[text, role]`: `b` opens a scope (effect known when run), `e` closes
/// it, `i` and `p` neither (`i`'s effect is pending too); every node reads
/// the scope kind (but `p`) and `x`, defines `x` with the value read
/// (dynamic, bounded to `x`) and contributes 1 to the sequence `items`.
struct Scoped;

fn role(v: &Values, a: &[ValueId]) -> String {
    a.get(1)
        .map_or_else(String::new, |&x| v.render(x).trim_matches('"').to_owned())
}

impl Operation for Scoped {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define(&self, _: &[ValueId], _: &Values) -> MayDefine {
        MayDefine::Prefixes(vec!["x".to_owned()])
    }
    fn reads(&self, _: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(vec![v.field("x")])
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> Effect {
        if role(v, a) == "p" {
            Effect::None
        } else {
            Effect::Pending
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let r = role(v, &a);
        if r != "p" {
            let _ = cx.scope_kind()?;
        }
        let x = cx.use_field(v.field("x"))?.unwrap_or_else(|| v.int(1));
        match r.as_str() {
            "b" => {
                cx.set_effect(Effect::Open);
                cx.set_kind(v.str("env"));
            }
            "e" => cx.set_effect(Effect::Close),
            "i" => cx.set_effect(Effect::None),
            _ => {}
        }
        cx.define_with(v.field("x"), x, Locality::Global);
        cx.push_contribution(v.field("items"), v.int(1));
        Ok(ValueId::UNIT)
    }
}

#[test]
fn an_edit_inside_a_scope_that_changes_nothing_runs_one_node() {
    let mut eng = Engine::new();
    eng.declare_sequence(eng.values().field("items"));
    let op: OpId = eng.register(Arc::new(Scoped));
    let u = eng.new_unit();
    let mut roles = Vec::new();
    for _ in 0..3 {
        roles.extend(["p", "p", "b", "i", "i", "i", "i", "e"]);
    }
    roles.extend(["p", "p"]);
    let nodes: Vec<NodeId> = roles
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let a = vec![eng.values().str(&format!("node {i}")), eng.values().str(r)];
            eng.push(u, op, a)
        })
        .collect();
    eng.evaluate_sequential().expect("cold");
    // (the second item of the first scope)
    let a = vec![eng.values().str("node 4, edited"), eng.values().str("i")];
    eng.replace(nodes[4], op, a);
    let r = eng.evaluate_sequential().expect("edit");
    eprintln!(
        "cascade scoped: {} nodes run, {} runs, {} cutoffs, {} rebinds (of {})",
        r.nodes_run,
        r.runs,
        r.cutoffs,
        r.rebinds,
        roles.len()
    );
    assert_eq!(r.nodes_run, 1);
}
