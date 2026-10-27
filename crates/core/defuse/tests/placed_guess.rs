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
