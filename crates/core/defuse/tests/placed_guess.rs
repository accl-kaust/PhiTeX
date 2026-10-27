//! A placer folding its placements' outputs goes on on a guess while one
//! has not run (`Ctx::placed_output_or`): a placement whose output is the
//! guess re-asks it for nothing, one whose output differs re-asks it once.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Cause, Ctx, Engine, Operation, Stop, ValueId};

/// `[x]`: outputs x.
struct Leaf;

impl Operation for Leaf {
    fn name(&self) -> &'static str {
        "leaf"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx.args()[0])
    }
}

/// `[a, b, guess]`: places leaves `[a]` and `[b]`, sums their outputs,
/// each read on `guess` while it has not run. Counts its runs and keeps
/// the cause of its last.
struct Placer {
    leaf: std::sync::OnceLock<phitex_defuse::OpId>,
    runs: AtomicU64,
    cause: std::sync::Mutex<Option<Cause>>,
}

impl Operation for Placer {
    fn name(&self) -> &'static str {
        "placer"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::Relaxed);
        *self.cause.lock().unwrap() = cx.woken_by();
        let v = cx.values();
        let (a, b, g) = (cx.args()[0], cx.args()[1], cx.args()[2]);
        let leaf = *self.leaf.get().unwrap();
        let i = cx.child(leaf, vec![a]);
        let j = cx.child(leaf, vec![b]);
        let x = v.as_int(cx.placed_output_or(i, g)).unwrap_or(0);
        let y = v.as_int(cx.placed_output_or(j, g)).unwrap_or(0);
        Ok(v.int(x + y))
    }
}

#[test]
fn a_placement_whose_output_is_the_guess_reasks_nobody() {
    for threads in [1, 4] {
        let e = Engine::new();
        let v = e.values();
        let leaf = e.register(Arc::new(Leaf));
        let p = Arc::new(Placer {
            leaf: std::sync::OnceLock::new(),
            runs: AtomicU64::new(0),
            cause: std::sync::Mutex::new(None),
        });
        p.leaf.set(leaf).unwrap();
        let op = e.register(p.clone());
        let u = e.new_unit();
        let n = e.push(u, op, vec![v.int(10), v.int(20), v.int(10)]);
        let r = e.evaluate(threads).unwrap();
        assert_eq!(e.output(n).and_then(|x| v.as_int(x)), Some(30), "-j{threads}: {r:?}");
        assert_eq!(p.runs.load(Ordering::Relaxed), 2, "-j{threads}: the first run on the guesses, one more woken by the leaf that gave 20: {r:?}");
        assert!(r.commit_by_op.iter().any(|c| c.0 == "placer" && c.2 == 4), "two commits of two placed reads each: {:?}", r.commit_by_op);
        let cause = p.cause.lock().unwrap().clone();
        assert!(matches!(cause, Some(Cause::Output { .. })), "-j{threads}: woken by the second leaf's output: {cause:?}");
        assert!(e.check_invariants().is_empty());
    }
}

#[test]
fn every_guess_right_runs_the_placer_once() {
    let e = Engine::new();
    let v = e.values();
    let leaf = e.register(Arc::new(Leaf));
    let p = Arc::new(Placer {
        leaf: std::sync::OnceLock::new(),
        runs: AtomicU64::new(0),
        cause: std::sync::Mutex::new(None),
    });
    p.leaf.set(leaf).unwrap();
    let op = e.register(p.clone());
    let u = e.new_unit();
    let n = e.push(u, op, vec![v.int(7), v.int(7), v.int(7)]);
    e.evaluate(2).unwrap();
    assert_eq!(e.output(n).and_then(|x| v.as_int(x)), Some(14));
    assert_eq!(p.runs.load(Ordering::Relaxed), 1);
    assert!(e.check_invariants().is_empty());
}

/// `[anchor, n, salt]`: places `n` leaves after node `anchor`, each after
/// the one before (`Ctx::follow_after_placed`), the first after the
/// anchor's own children.
struct Chain {
    leaf: std::sync::OnceLock<phitex_defuse::OpId>,
}

impl Operation for Chain {
    fn name(&self) -> &'static str {
        "chain"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let at = phitex_defuse::NodeId::from_raw(v.as_int(cx.args()[0]).unwrap_or(0).unsigned_abs());
        let n = v.as_int(cx.args()[1]).unwrap_or(0);
        let salt = v.as_int(cx.args()[2]).unwrap_or(0);
        let leaf = *self.leaf.get().unwrap();
        let mut last = cx.follow_after(at, leaf, vec![v.int(salt * 100)]);
        for k in 1..n {
            last = cx.follow_after_placed(last, leaf, vec![v.int(salt * 100 + k)]);
        }
        Ok(ValueId::UNIT)
    }
}

/// Several nodes placed after one node elsewhere stand in the placer's
/// order (each after the one before), on the first run and after a run
/// that keeps some and makes others anew.
#[test]
fn placements_after_a_placement_stand_in_order() {
    let e = Engine::new();
    let v = e.values();
    let leaf = e.register(Arc::new(Leaf));
    let c = Arc::new(Chain { leaf: std::sync::OnceLock::new() });
    c.leaf.set(leaf).unwrap();
    let op = e.register(c);
    let u = e.new_unit();
    let a = e.push(u, leaf, vec![v.int(-1)]);
    let b = e.push(u, leaf, vec![v.int(-2)]);
    let ch = e.push(u, op, vec![v.int(i64::try_from(a.raw()).unwrap()), v.int(3), v.int(1)]);
    let r = e.evaluate(2).unwrap();
    assert!(r.placed_by_op.contains(&("leaf", 3)), "three placements made: {:?}", r.placed_by_op);
    let outs = |e: &Engine| -> Vec<i64> { e.order(u).into_iter().filter_map(|n| e.output(n).and_then(|x| v.as_int(x))).collect() };
    assert_eq!(outs(&e), vec![-1, 100, 101, 102, -2], "after the anchor, in the placer's order");
    // (the middle one made anew: the others kept, the order the same)
    e.replace(ch, op, vec![v.int(i64::try_from(a.raw()).unwrap()), v.int(4), v.int(1)]);
    let r = e.evaluate(2).unwrap();
    assert!(r.placed_by_op.contains(&("leaf", 1)), "one placement made, three kept: {:?}", r.placed_by_op);
    assert_eq!(outs(&e), vec![-1, 100, 101, 102, 103, -2]);
    let _ = b;
    assert!(e.check_invariants().is_empty());
}

/// `[target]`: 1 where the node `target` (raw id) still stands, else 0.
struct Alive;

impl Operation for Alive {
    fn name(&self) -> &'static str {
        "alive"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let t = phitex_defuse::NodeId::from_raw(v.as_int(cx.args()[0]).unwrap_or(0).unsigned_abs());
        Ok(v.int(i64::from(cx.alive(t))))
    }
}

/// A run asks whether a node still stands before it places after it
/// (`Ctx::alive`): a node removed by an edit does not.
#[test]
fn a_run_knows_whether_a_node_still_stands() {
    let e = Engine::new();
    let v = e.values();
    let alive = e.register(Arc::new(Alive));
    let u = e.new_unit();
    let a = e.push(u, alive, vec![v.int(0)]);
    let p = e.push(u, alive, vec![v.int(i64::try_from(a.raw()).unwrap())]);
    e.evaluate(2).unwrap();
    assert_eq!(e.output(p).and_then(|x| v.as_int(x)), Some(1));
    e.remove(a);
    e.replace(p, alive, vec![v.int(i64::try_from(a.raw()).unwrap())]);
    e.evaluate(2).unwrap();
    assert_eq!(e.output(p).and_then(|x| v.as_int(x)), Some(0));
}
