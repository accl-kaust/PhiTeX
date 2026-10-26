//! Reads taken back and relations not shared: what a client says of its
//! own reads and of its operation's nodes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, OpId, Operation, Report, Stop, ValueId};

/// `[s, x]`: contributes x to s.
struct Contrib;

impl Operation for Contrib {
    fn name(&self) -> &'static str {
        "contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.contribute(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[s]`: folds s and the next contribution to it, then takes both reads
/// back: its output does not depend on them.
struct Unread(AtomicU64);

impl Operation for Unread {
    fn name(&self) -> &'static str {
        "unread"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let s = Field(cx.args()[0]);
        cx.fold(s, None, None)?;
        cx.next_contribution(s, None)?;
        cx.unread(s);
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(ValueId::UNIT)
    }
}

/// `[f, k]`: f := f + 1 as a relation where k is 0, else defines nothing:
/// each node its own text.
struct Text;

impl Operation for Text {
    fn name(&self) -> &'static str {
        "text"
    }

    fn alike(&self) -> bool {
        false
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        if cx.values().as_int(cx.args()[1]) == Some(0) {
            cx.define_rel(Field(cx.args()[0]), 1);
        }
        Ok(ValueId::UNIT)
    }
}

/// `[f]`: the definition of f reaching it.
struct Get;

impl Operation for Get {
    fn name(&self) -> &'static str {
        "get"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
    }
}

#[test]
fn a_fold_and_a_next_taken_back_are_not_asked_again() {
    let e = Engine::new();
    let v = e.values();
    let contrib = e.register(Arc::new(Contrib));
    let unread = Arc::new(Unread(AtomicU64::new(0)));
    let u_op: OpId = e.register(unread.clone());
    let s = v.field("s");
    let u = e.new_unit();
    let c = e.push(u, contrib, vec![s.0, v.int(1)]);
    e.push(u, u_op, vec![s.0]);
    e.evaluate(2).unwrap();
    assert_eq!(unread.0.load(Ordering::Relaxed), 1);
    e.replace(c, contrib, vec![s.0, v.int(2)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(Report::by_op(&r.runs_by_op, "unread"), 0, "its reads of s were taken back: {r:?}");
    assert_eq!(unread.0.load(Ordering::Relaxed), 1);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn an_operation_whose_nodes_differ_shapes_none() {
    let e = Engine::new();
    let v = e.values();
    let text = e.register(Arc::new(Text));
    let get = e.register(Arc::new(Get));
    let f = v.field("c");
    let u = e.new_unit();
    let mut gets = Vec::new();
    for i in 0..50 {
        e.push(u, text, vec![f.0, v.int(i64::from(i != 0))]);
        gets.push(e.push(u, get, vec![f.0]));
    }
    let r = e.evaluate_sequential().unwrap();
    assert_eq!(r.shaped, 0, "one node's relation is no other's: {r:?}");
    for g in gets {
        assert_eq!(e.output(g).and_then(|x| v.as_int(x)), Some(1));
    }
    assert!(e.check_invariants().is_empty());
}
