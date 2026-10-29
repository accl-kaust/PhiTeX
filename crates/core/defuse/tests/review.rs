//! The reviewer's list: each a shape that cost a square, a global lock or
//! a run for ever, now bounded.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{
    Ctx, EditSource, Engine, Field, Limits, Monoid, NodeId, OpId, Operation, Stop, SumMonoid, UnitId, ValueId, Values,
};

/// `[f, x]`: contributes x to f.
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

/// `[f]`: the fold of f from the unit's start to it.
struct FoldAll;

impl Operation for FoldAll {
    fn name(&self) -> &'static str {
        "fold_all"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.fold(Field(cx.args()[0]), None, None)
    }
}

/// `[f, from]`: the fold of f after node `from` up to it.
struct FoldFrom;

impl Operation for FoldFrom {
    fn name(&self) -> &'static str {
        "fold_from"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let from = NodeId::from_raw(cx.values().as_int(cx.args()[1]).unwrap() as u64);
        cx.fold(Field(cx.args()[0]), Some(from), None)
    }
}

/// `[f]`: the first contribution to f after the unit's start, its value.
struct First;

impl Operation for First {
    fn name(&self) -> &'static str {
        "first"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx
            .next_contribution(Field(cx.args()[0]), None)?
            .map_or(ValueId::UNIT, |x| x.1))
    }
}

/// `[]`: nothing, and never contributes to anything until it has run
/// (its default: it may).
struct Quiet;

impl Operation for Quiet {
    fn name(&self) -> &'static str {
        "quiet"
    }

    fn run(&self, _cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

/// `[f, k]`: the environment's f; counts its runs.
struct EnvGet(AtomicU64);

impl Operation for EnvGet {
    fn name(&self) -> &'static str {
        "env_get"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(cx.env(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
    }
}

fn int(e: &Engine, n: NodeId) -> i64 {
    e.values().as_int(e.output(n).expect("ran")).expect("int")
}

#[test]
fn an_environment_reader_run_again_and_again_is_asked_once() {
    // Its readers are filed by field beside the range readers, the stale
    // ones dropped as the list grows: twenty thousand runs of one reader
    // leave one reader to ask.
    let e = Engine::new();
    let get = Arc::new(EnvGet(AtomicU64::new(0)));
    let op = e.register(get.clone());
    let v = e.values();
    let x = v.field("x");
    e.set_environment([(x, v.int(1))]);
    let u = e.new_unit();
    let n = e.push(u, op, vec![x.0, v.int(0)]);
    e.evaluate(1).unwrap();
    for i in 0..20_000 {
        e.replace(n, op, vec![x.0, v.int(i)]);
        e.evaluate(1).unwrap();
    }
    let before = get.0.load(Ordering::Relaxed);
    let t = Instant::now();
    e.update_environment(&[(x, Some(v.int(2)))]);
    assert!(t.elapsed() < Duration::from_millis(20), "{:?}", t.elapsed());
    e.evaluate(1).unwrap();
    assert_eq!(get.0.load(Ordering::Relaxed), before + 1);
    assert_eq!(int(&e, n), 2);
}

#[test]
fn folds_from_the_start_cost_what_holds_a_change() {
    // Five thousand folds from the unit's start, then five thousand
    // contributions after them: a contribution asks for the folds whose
    // interval holds it by their ends, not by their common start.
    let e = Engine::new();
    let c = e.register(Arc::new(Contrib));
    let fa = e.register(Arc::new(FoldAll));
    let v = e.values();
    let s = v.field("s");
    e.declare_accumulator(s, Arc::new(SumMonoid));
    let u = e.new_unit();
    let readers: Vec<NodeId> = (0..5000).map(|_| e.push(u, fa, vec![s.0])).collect();
    for _ in 0..5000 {
        e.push(u, c, vec![s.0, v.int(1)]);
    }
    let last = e.push(u, fa, vec![s.0]);
    let t = Instant::now();
    e.evaluate(1).unwrap();
    assert!(t.elapsed() < Duration::from_millis(400), "{:?}", t.elapsed());
    assert_eq!(int(&e, readers[4999]), 0);
    assert_eq!(int(&e, last), 5000);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_next_past_many_holes_goes_on_from_each() {
    // The first contribution searched from the start stops at each node
    // not yet run; each that commits with nothing moves the search on
    // from itself, not from the start again.
    let e = Engine::new();
    let first = e.register(Arc::new(First));
    let quiet = e.register(Arc::new(Quiet));
    let c = e.register(Arc::new(Contrib));
    let v = e.values();
    let s = v.field("s");
    let u = e.new_unit();
    let r = e.push(u, first, vec![s.0]);
    for _ in 0..20_000 {
        e.push(u, quiet, vec![]);
    }
    e.push(u, c, vec![s.0, v.int(7)]);
    let t = Instant::now();
    e.evaluate(1).unwrap();
    assert!(t.elapsed() < Duration::from_millis(1500), "{:?}", t.elapsed());
    assert_eq!(int(&e, r), 7);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_fold_from_a_removed_node_answers_and_stops() {
    // The node a fold starts after is removed: the fold answers empty (as
    // a read at a gone node answers gone) instead of stopping its run for
    // ever.
    let e = Engine::new();
    e.set_limits(Limits {
        runs_per_node: 100,
        ..Limits::default()
    });
    let c = e.register(Arc::new(Contrib));
    let ff = e.register(Arc::new(FoldFrom));
    let v = e.values();
    let s = v.field("s");
    e.declare_accumulator(s, Arc::new(SumMonoid));
    let u = e.new_unit();
    let a = e.push(u, c, vec![s.0, v.int(1)]);
    e.push(u, c, vec![s.0, v.int(2)]);
    let r = e.push(u, ff, vec![s.0, v.int(a.raw() as i64)]);
    e.evaluate(2).unwrap();
    assert_eq!(int(&e, r), 2);
    e.remove(a);
    e.evaluate(2).expect("the reader settles");
    assert_eq!(int(&e, r), 0);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_fold_from_a_node_after_its_reader_answers_empty() {
    // The node a fold starts after stands after the reader, segments of
    // contributions between them: nothing lies between the bounds, so the
    // fold answers empty. It searched the segments from the later one to
    // the earlier, which panicked; the worker died with the node counted
    // outstanding and the other workers waited for ever.
    let e = Engine::new();
    let c = e.register(Arc::new(Contrib));
    let q = e.register(Arc::new(Quiet));
    let ff = e.register(Arc::new(FoldFrom));
    let v = e.values();
    let s = v.field("s");
    e.declare_accumulator(s, Arc::new(SumMonoid));
    let u = e.new_unit();
    let r = e.push(u, q, vec![]);
    let mut last = r;
    for _ in 0..1000 {
        last = e.push(u, c, vec![s.0, v.int(1)]);
    }
    e.evaluate(4).unwrap();
    e.replace(r, ff, vec![s.0, v.int(last.raw() as i64)]);
    e.evaluate(4).expect("the reader runs");
    assert_eq!(int(&e, r), 0);
    assert!(e.check_invariants().is_empty());
}

/// Sums, and panics on a -7: a fold that breaks wherever it is asked.
struct SevenPanics;

impl Monoid for SevenPanics {
    fn empty(&self, v: &Values) -> ValueId {
        v.int(0)
    }

    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        let (a, b) = (v.as_int(a).unwrap_or(0), v.as_int(b).unwrap_or(0));
        assert!(a != -7 && b != -7, "a minus seven");
        v.int(a + b)
    }
}

#[test]
fn a_panic_outside_a_run_ends_the_evaluation() {
    // A contribution whose fold panics where a commit re-asks its readers,
    // on the worker outside any run: the evaluation ends with the error.
    // The worker died with the node it committed counted outstanding, and
    // the others waited for it for ever.
    let e = Arc::new(Engine::new());
    let c = e.register(Arc::new(Contrib));
    let fa = e.register(Arc::new(FoldAll));
    let v = e.values();
    let s = v.field("s");
    e.declare_accumulator(s, Arc::new(SevenPanics));
    let u = e.new_unit();
    let a = e.push(u, c, vec![s.0, v.int(1)]);
    for _ in 0..200 {
        e.push(u, c, vec![s.0, v.int(1)]);
    }
    for _ in 0..50 {
        e.push(u, fa, vec![s.0]);
    }
    e.evaluate(4).unwrap();
    e.replace(a, c, vec![s.0, v.int(-7)]);
    let (tx, rx) = std::sync::mpsc::channel();
    let e2 = e.clone();
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| e2.evaluate(4).is_err()));
        let _ = tx.send(r);
    });
    let r = rx.recv_timeout(Duration::from_secs(30)).expect("the evaluation ends");
    assert_eq!(r.ok(), Some(true), "it ends with an error");
}

/// Sends one edit, version 5, the first time it is asked: a node more.
struct OneEdit(UnitId, OpId, Field, bool);

impl EditSource for OneEdit {
    fn absorb(&mut self, eng: &Engine) -> Option<u64> {
        if std::mem::replace(&mut self.3, true) {
            return None;
        }
        let v = eng.values();
        eng.push(self.0, self.1, vec![self.2.0, v.int(5)]);
        Some(5)
    }
}

#[test]
fn an_edit_taken_during_an_evaluation_is_its_version() {
    // The edit taken while the evaluation runs (not before it) sets the
    // version the report gives, and the report lists it with the runs
    // made before it: the version was left as it was.
    let e = Engine::new();
    let c = e.register(Arc::new(Contrib));
    let fa = e.register(Arc::new(FoldAll));
    let v = e.values();
    let s = v.field("s");
    e.declare_accumulator(s, Arc::new(SumMonoid));
    let u = e.new_unit();
    for _ in 0..50 {
        e.push(u, c, vec![s.0, v.int(1)]);
    }
    let r = e.push(u, fa, vec![s.0]);
    let mut src = OneEdit(u, c, s, false);
    let rep = e.evaluate_absorbing(1, &mut src).unwrap();
    assert_eq!(rep.version, 5);
    assert_eq!(rep.absorbed.len(), 1, "{:?}", rep.absorbed);
    assert_eq!(rep.absorbed[0].0, 5);
    assert!(rep.absorbed[0].1 > 0, "taken after runs: {:?}", rep.absorbed);
    assert_eq!(int(&e, r), 50);
}

/// `[n]`: places `n` children `Quiet` after itself and contributes 1 to
/// `s` (a piece whose lines split its segment).
struct Splitter(OpId);

impl Operation for Splitter {
    fn name(&self) -> &'static str {
        "splitter"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let n = cx.values().as_int(cx.args()[0]).unwrap();
        for _ in 0..n {
            cx.child(self.0, vec![]);
        }
        Ok(cx.args()[0])
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

#[test]
fn next_readers_follow_their_stops_across_splits() {
    // `next` readers stop at nodes not yet run while placements before
    // those nodes split their segments under them; a stop is found where
    // its node is, under that segment's lock, when the node commits, is
    // taken or goes. Many threads, many times.
    for round in 0..20 {
        let e = Engine::new();
        let quiet = e.register(Arc::new(Quiet));
        let split = e.register(Arc::new(Splitter(quiet)));
        let first = e.register(Arc::new(First));
        let c = e.register(Arc::new(Contrib));
        let v = e.values();
        let s = v.field("s");
        let u = e.new_unit();
        let mut rs = Vec::new();
        for i in 0..200 {
            rs.push(e.push(u, first, vec![s.0]));
            e.push(u, split, vec![v.int(40)]);
            e.push(u, quiet, vec![]);
            if i == 150 {
                e.push(u, c, vec![s.0, v.int(round)]);
            }
        }
        e.evaluate(16).unwrap();
        for r in rs {
            assert_eq!(int(&e, r), round);
        }
        assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
    }
}
