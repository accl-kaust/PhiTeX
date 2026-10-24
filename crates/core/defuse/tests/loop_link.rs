//! A read of a loop-back field's round value is linked in the chain to
//! the commit that ended the round before (`Cause::Loop`): a rerun for the
//! next round waited for it. Unit [p, r]: `p` reads the loop-back field
//! `n` and publishes `n + 1` up to 2; `r` reads `n`.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops;

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let n = v.field("n");
        let x = cx.use_field(n)?.and_then(|y| v.as_int(y)).unwrap_or(0);
        if v.as_str(a[0]).as_deref() == Some("p") {
            cx.publish(n, v.int((x + 1).min(2)));
        }
        Ok(v.int(x))
    }
}

#[test]
fn a_round_value_links_to_the_round_end() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    eng.set_heights(true);
    let (n, zero) = (eng.values().field("n"), eng.values().int(0));
    eng.declare_loopback(n, zero);
    let u = eng.new_unit();
    let p = vec![eng.values().str("p")];
    eng.push(u, op, p);
    let r = vec![eng.values().str("r")];
    let r = eng.push(u, op, r);
    let rep = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(eng.output(r).and_then(|y| eng.values().as_int(y)), Some(2));
    assert!(rep.rounds >= 3, "rounds {}", rep.rounds);
    let chain = eng.critical_chain_by_time();
    assert!(chain.iter().any(|(_, why, _)| matches!(why, phitex_defuse::Cause::Loop { field } if *field == n)), "{chain:?}");
}

/// A field a run declares a loop-back: its readers run again on its round
/// value, linked to the declaring run (`Cause::Loop`). Unit [r, d]: `r`
/// reads `m`; `d` declares `m` a loop-back (initial 4). And a reader of
/// one of two fields that moved runs again for the one it read: unit
/// [p, q, s] where `p` publishes `a` and `b`, `s` reads `b` only.
struct Late;

impl Operation for Late {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match v.as_str(a[0]).as_deref() {
            Some("d") => {
                cx.declare_loopback(v.field("m"), v.int(4));
                Ok(ValueId::UNIT)
            }
            Some("p") => {
                let (fa, fb) = (v.field("a"), v.field("b"));
                cx.declare_loopback(fa, v.int(0));
                cx.declare_loopback(fb, v.int(0));
                cx.publish(fa, v.int(1));
                cx.publish(fb, v.int(1));
                Ok(ValueId::UNIT)
            }
            Some("s") => Ok(cx.use_field(v.field(&v.as_str(a[1]).unwrap()))?.unwrap_or_else(|| v.int(-1))),
            _ => Ok(cx.use_field(v.field("m"))?.unwrap_or_else(|| v.int(-1))),
        }
    }
}

#[test]
fn a_declared_loopback_links_its_readers_to_the_declaring_run() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Late));
    eng.set_heights(true);
    let u = eng.new_unit();
    let r = vec![eng.values().str("r")];
    let r = eng.push(u, op, r);
    let d = vec![eng.values().str("d")];
    let d = eng.push(u, op, d);
    eng.evaluate_sequential().expect("evaluate");
    assert_eq!(eng.output(r).and_then(|y| eng.values().as_int(y)), Some(4));
    let m = eng.values().field("m");
    let chain = eng.critical_chain_by_time();
    let at = chain.iter().position(|(_, why, _)| matches!(why, phitex_defuse::Cause::Loop { field } if *field == m));
    assert!(at.is_some_and(|i| chain.get(i + 1).is_some_and(|c| c.0 == d)), "{chain:?}");
}

#[test]
fn a_loop_rerun_names_the_field_its_reader_read() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Late));
    let u = eng.new_unit();
    // (`s` reads the one of `a`, `b` that moves second, in the engine's
    // order of fields: its rerun names that one, not the first.)
    let (fa, fb) = (eng.values().field("a"), eng.values().field("b"));
    let (name, f) = if fa.0.raw() < fb.0.raw() { ("b", fb) } else { ("a", fa) };
    let p = vec![eng.values().str("p")];
    eng.push(u, op, p);
    let a = vec![eng.values().str("s"), eng.values().str(name)];
    let s = eng.push(u, op, a);
    let rep = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(eng.output(s).and_then(|y| eng.values().as_int(y)), Some(1));
    let why: Vec<_> = rep.rerun_causes.iter().filter(|c| c.0 == s).map(|c| c.1).collect();
    assert!(why.contains(&phitex_defuse::Rerun::Loop { field: f }), "{why:?}");
}
