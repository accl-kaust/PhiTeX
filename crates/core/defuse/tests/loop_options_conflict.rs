//! Conflicting loop-back declarations (early or not, last wins or not):
//! with the check on (`Engine::set_loop_option_check`), an error naming
//! the field and both declarers; off, the field is early, and last wins,
//! if any declaration says so.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, EvalError, Field, Locality, LoopOptions, NodeId, Operation, Stop, ValueId, Values};

/// `decl k e`: declares `f` with early `e` (k: which node it is).
struct Ops;

impl Operation for Ops {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let early = v.as_bool(a[0]) == Some(true);
        cx.declare_loopback_with(v.field("f"), v.int(0), LoopOptions { early, last_wins: false });
        Ok(ValueId::UNIT)
    }
}

fn engine(check: bool) -> (Engine, phitex_defuse::OpId, phitex_defuse::UnitId) {
    let mut eng = Engine::new();
    eng.set_loop_option_check(check);
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    (eng, op, u)
}

#[test]
fn an_api_declaration_conflicting_is_an_error() {
    let (mut eng, _, _) = engine(true);
    let (f, zero) = (eng.values().field("f"), eng.values().int(0));
    eng.declare_loopback(f, zero);
    eng.declare_loopback_early(f, zero);
    match eng.evaluate_sequential() {
        Err(EvalError::ConflictingLoopOptions { field, first: None, by: None, first_options, options }) => {
            assert_eq!((field, first_options.early, options.early), (f, false, true));
        }
        r => panic!("{r:?}"),
    }
}

#[test]
fn runs_declaring_conflicting_options_are_named() {
    let (mut eng, op, u) = engine(true);
    let (no, yes) = (eng.values().bool(false), eng.values().bool(true));
    let a = eng.push(u, op, vec![no]);
    let b = eng.push(u, op, vec![yes]);
    match eng.evaluate_sequential() {
        Err(EvalError::ConflictingLoopOptions { first: Some(x), by: Some(y), .. }) => {
            let mut got = [x, y];
            got.sort();
            let mut want: [NodeId; 2] = [a, b];
            want.sort();
            assert_eq!(got, want);
        }
        r => panic!("{r:?}"),
    }
}

#[test]
fn the_same_options_again_are_fine_and_unchecked_the_upgrade_stands() {
    let (mut eng, op, u) = engine(true);
    let yes = eng.values().bool(true);
    eng.push(u, op, vec![yes]);
    eng.push(u, op, vec![yes]);
    eng.evaluate_sequential().expect("the same options twice");
    let (mut eng, op, u) = engine(false);
    let (no, yes) = (eng.values().bool(false), eng.values().bool(true));
    eng.push(u, op, vec![no]);
    eng.push(u, op, vec![yes]);
    eng.evaluate_sequential().expect("unchecked: early, as any declaration says");
}
