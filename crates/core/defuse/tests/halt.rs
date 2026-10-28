//! `Ctx::halt`: a run ends the evaluation once it has committed; nothing
//! queued runs, the nodes still queued are pending for the next
//! evaluation, and `Report::halted` says so.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, Operation, Stop, ValueId};

/// `[name, x, halt]`: defines `name` := x; with `halt` != 0 it ends the
/// evaluation. Counts its runs.
struct Def {
    runs: AtomicU64,
}

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let a = cx.args().to_vec();
        cx.define(Field(a[0]), a[1]);
        if v.as_int(a[2]).unwrap_or(0) != 0 {
            cx.halt();
        }
        Ok(a[1])
    }
}

#[test]
fn a_halting_run_ends_the_evaluation_and_leaves_the_rest_pending() {
    let e = Engine::new();
    let op = Arc::new(Def { runs: AtomicU64::new(0) });
    let def = e.register(op.clone());
    let v = e.values();
    let u = e.new_unit();
    let a = e.push(u, def, vec![v.field("a").0, v.int(1), v.int(1)]);
    let rest: Vec<_> = (0..50).map(|i| e.push(u, def, vec![v.field(&format!("b{i}")).0, v.int(i), v.int(0)])).collect();
    let rep = e.evaluate(1).unwrap();
    assert!(rep.halted, "{rep:?}");
    assert_eq!(e.output(a).and_then(|x| v.as_int(x)), Some(1), "the halting run committed: {rep:?}");
    assert_eq!(op.runs.load(Ordering::SeqCst), 1, "nothing after it ran: {rep:?}");
    assert!(rest.iter().all(|&n| e.output(n).is_none()), "the rest has no output yet");
    // the halting node no longer halts: the next evaluation runs the rest
    e.replace(a, def, vec![v.field("a").0, v.int(2), v.int(0)]);
    let rep = e.evaluate(4).unwrap();
    assert!(!rep.halted, "{rep:?}");
    assert_eq!(e.output(a).and_then(|x| v.as_int(x)), Some(2));
    for (i, &n) in rest.iter().enumerate() {
        assert_eq!(e.output(n).and_then(|x| v.as_int(x)), Some(i as i64), "node {i}: {rep:?}");
    }
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// Eight workers: the runs after the halting one that had started may
/// finish, but every evaluation ends, every node not run is pending, and
/// the next evaluation completes them.
#[test]
fn a_halt_on_many_workers_leaves_a_consistent_line() {
    let e = Engine::new();
    let op = Arc::new(Def { runs: AtomicU64::new(0) });
    let def = e.register(op.clone());
    let v = e.values();
    let u = e.new_unit();
    let nodes: Vec<_> = (0..400).map(|i| e.push(u, def, vec![v.field(&format!("c{i}")).0, v.int(i), v.int(i64::from(i == 100))])).collect();
    let rep = e.evaluate(8).unwrap();
    assert!(rep.halted, "{rep:?}");
    e.replace(nodes[100], def, vec![v.field("c100").0, v.int(100), v.int(0)]);
    let rep = e.evaluate(8).unwrap();
    assert!(!rep.halted, "{rep:?}");
    for (i, &n) in nodes.iter().enumerate() {
        assert_eq!(e.output(n).and_then(|x| v.as_int(x)), Some(i as i64), "node {i}");
    }
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}
