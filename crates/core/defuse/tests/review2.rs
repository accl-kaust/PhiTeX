//! The second review: placement order across segments, predicted
//! relations, reader sweeps, inverted folds, edits taken mid-evaluation.
//! Each test a fault found, failing before its fix.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, NodeId, OpId, Operation, Report, Stop, ValueId};

/// `[x]`: defines "x" = x.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.args()[0];
        cx.define(cx.values().field("x"), x);
        Ok(x)
    }
}

/// `[x]`: defines "t" = x.
struct DefT;

impl Operation for DefT {
    fn name(&self) -> &'static str {
        "def_t"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.args()[0];
        cx.define(cx.values().field("t"), x);
        Ok(x)
    }
}

/// `[]`: nothing.
struct Quiet;

impl Operation for Quiet {
    fn name(&self) -> &'static str {
        "quiet"
    }

    fn run(&self, _cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

fn raw(e: &Engine, n: NodeId) -> ValueId {
    e.values().int(i64::try_from(n.raw()).unwrap())
}

fn node_arg(cx: &Ctx<'_>, i: usize) -> NodeId {
    NodeId::from_raw(cx.values().as_int(cx.args()[i]).unwrap_or(0).unsigned_abs())
}

/// `[a]`: reads "t"; places a `Def` of 7 after node `a` and its children,
/// then a `Def` of 8 after that one (a page step's finisher and the step
/// after it).
struct PlaceAfter(OpId);

impl Operation for PlaceAfter {
    fn name(&self) -> &'static str {
        "place_after"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        cx.read(v.field("t"))?;
        let at = node_arg(cx, 0);
        cx.follow_after(at, self.0, vec![v.int(7)]);
        cx.follow(self.0, vec![v.int(8)]);
        Ok(ValueId::UNIT)
    }
}

/// `[n]`: reads "t"; places `n` `Quiet` children, then a `Def` of 9 as its
/// continuation (a piece's lines, then its rest).
struct Lines(OpId, OpId);

impl Operation for Lines {
    fn name(&self) -> &'static str {
        "lines"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        cx.read(v.field("t"))?;
        let n = v.as_int(cx.args()[0]).unwrap_or(0);
        for _ in 0..n {
            cx.child(self.0, vec![]);
        }
        cx.follow(self.1, vec![v.int(9)]);
        Ok(ValueId::UNIT)
    }
}

fn pos(o: &[NodeId], n: NodeId) -> usize {
    o.iter().position(|&m| m == n).unwrap()
}

#[test]
fn a_kept_placement_in_another_segment_is_not_stranded_by_nodes_after_it() {
    // The placer stands segments after the node it places after, with
    // nodes pushed after it in its own segment. Its placements, kept where
    // they stand when it runs again, were checked against every node of
    // the placer's segment after their floor, past the placement itself:
    // the nodes after the placer counted as standing between, and both
    // placements were made anew at every run of the placer.
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let def_t = e.register(Arc::new(DefT));
    let quiet = e.register(Arc::new(Quiet));
    let back = e.register(Arc::new(PlaceAfter(def)));
    let u = e.new_unit();
    let t = e.push(u, def_t, vec![v.int(0)]);
    let a = e.push(u, def, vec![v.int(1)]);
    for _ in 0..200 {
        e.push(u, quiet, vec![]);
    }
    let p = e.push(u, back, vec![raw(&e, a)]);
    for _ in 0..10 {
        e.push(u, quiet, vec![]);
    }
    e.evaluate(2).unwrap();
    let kids = e.children(p);
    assert_eq!(kids.len(), 2);
    let order = e.order(u);
    assert!(pos(&order, a) + 1 == pos(&order, kids[0]) && pos(&order, kids[0]) + 1 == pos(&order, kids[1]), "{order:?}");
    e.replace(t, def_t, vec![v.int(1)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(e.children(p), kids, "the placer ran again and placed the same: {r:?}");
    assert_eq!(Report::by_op(&r.runs_by_op, "def"), 0, "nothing made anew runs: {r:?}");
    assert!(e.check_invariants().is_empty());
}

/// Where the continuation of a placer of `n` lines stands after a node is
/// inserted right before it and the placer runs again: right after the
/// last line (true) or after the inserted node.
fn continuation_after_lines(n: i64) -> bool {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let def_t = e.register(Arc::new(DefT));
    let quiet = e.register(Arc::new(Quiet));
    let lines = e.register(Arc::new(Lines(quiet, def)));
    let u = e.new_unit();
    let t = e.push(u, def_t, vec![v.int(0)]);
    let p = e.push(u, lines, vec![v.int(n)]);
    e.push(u, quiet, vec![]);
    e.evaluate(2).unwrap();
    let kids = e.children(p);
    let c = *kids.last().unwrap();
    let z = e.insert_before(c, quiet, vec![]);
    e.evaluate(2).unwrap();
    e.replace(t, def_t, vec![v.int(1)]);
    e.evaluate(2).unwrap();
    assert!(e.check_invariants().is_empty());
    let kids = e.children(p);
    let c = *kids.last().unwrap();
    let last_line = kids[kids.len() - 2];
    let order = e.order(u);
    let right_after = pos(&order, last_line) + 1 == pos(&order, c);
    let after_z = pos(&order, z) < pos(&order, c);
    assert!(right_after != after_z, "{order:?}");
    right_after
}

#[test]
fn a_continuation_stands_where_it_would_in_one_segment() {
    // A node pushed between a placer's lines and its continuation: when the
    // placer runs again the continuation is remade after its last line (a
    // node of no slot under the placer stands between). Where the lines
    // fill several segments the check saw only the placer's own segment
    // and kept the continuation behind the pushed node: the line's order
    // depended on how it was cut into segments.
    let one = continuation_after_lines(3);
    let many = continuation_after_lines(300);
    assert_eq!(one, many, "the same order in one segment ({one}) and across many ({many})");
}

/// Sums, and panics while its flag is up: a client's fold that breaks
/// for a while (a bug fixed by the next edit).
struct Flaky(Arc<std::sync::atomic::AtomicBool>);

impl phitex_defuse::Monoid for Flaky {
    fn empty(&self, v: &phitex_defuse::Values) -> ValueId {
        v.int(0)
    }

    fn combine(&self, v: &phitex_defuse::Values, a: ValueId, b: ValueId) -> ValueId {
        assert!(!self.0.load(std::sync::atomic::Ordering::Relaxed), "the fold is broken");
        v.int(v.as_int(a).unwrap_or(0) + v.as_int(b).unwrap_or(0))
    }
}

/// `[s, x]`: contributes x to s.
struct Contrib;

impl Operation for Contrib {
    fn name(&self) -> &'static str {
        "contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.contribute(phitex_defuse::Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[s]`: the fold of s from the unit's start to it.
struct FoldAll;

impl Operation for FoldAll {
    fn name(&self) -> &'static str {
        "fold_all"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.fold(phitex_defuse::Field(cx.args()[0]), None, None)
    }
}

/// `[s]`: reads "t"; contributes it to s and places a `Def` of it.
struct PlaceT(OpId);

impl Operation for PlaceT {
    fn name(&self) -> &'static str {
        "place_t"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let t = cx.read(v.field("t"))?.unwrap_or(v.int(0));
        cx.contribute(phitex_defuse::Field(cx.args()[0]), t);
        cx.child(self.0, vec![t]);
        Ok(t)
    }
}

#[test]
fn a_commit_a_panic_cut_short_is_finished_by_the_next_evaluation() {
    // A client's fold panics where a commit re-asks the readers of the
    // contribution it changed, after the commit has filed its products
    // and placed a child: the evaluation ends with the error. Once the
    // fold works again, the next evaluation ran the node again, found its
    // products as filed and asked nobody: the child it placed never ran
    // and the fold after it kept the old sum.
    let e = Engine::new();
    let v = e.values();
    let broken = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s = v.field("s");
    e.declare_accumulator(s, Arc::new(Flaky(broken.clone())));
    let def = e.register(Arc::new(Def));
    let def_t = e.register(Arc::new(DefT));
    let contrib = e.register(Arc::new(Contrib));
    let fold = e.register(Arc::new(FoldAll));
    let place = e.register(Arc::new(PlaceT(def)));
    let u = e.new_unit();
    let t = e.push(u, def_t, vec![v.int(0)]);
    e.push(u, contrib, vec![s.0, v.int(1)]);
    let a = e.push(u, place, vec![s.0]);
    let f = e.push(u, fold, vec![s.0]);
    e.evaluate(1).unwrap();
    assert_eq!(e.output(f).and_then(|x| v.as_int(x)), Some(1));
    broken.store(true, std::sync::atomic::Ordering::Relaxed);
    e.replace(t, def_t, vec![v.int(5)]);
    assert!(e.evaluate(1).is_err(), "the fold panics");
    broken.store(false, std::sync::atomic::Ordering::Relaxed);
    let r = e.evaluate(1).unwrap();
    assert!(e.check_invariants().is_empty(), "{:?} {r:?}", e.check_invariants());
    assert_eq!(e.output(f).and_then(|x| v.as_int(x)), Some(6), "the fold has the new contribution");
    let k = e.children(a);
    assert_eq!(k.len(), 1);
    assert_eq!(e.output(k[0]).and_then(|x| v.as_int(x)), Some(5), "the child placed ran");
}

/// `[s]`: folds s, then panics: an operation broken on its input.
struct FoldThenPanic;

impl Operation for FoldThenPanic {
    fn name(&self) -> &'static str {
        "fold_then_panic"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.fold(phitex_defuse::Field(cx.args()[0]), None, None)?;
        panic!("broken on its input");
    }
}

/// `[]`: breaks the fold (a client's state a run changes).
struct Breaks(Arc<std::sync::atomic::AtomicBool>);

impl Operation for Breaks {
    fn name(&self) -> &'static str {
        "breaks"
    }

    fn run(&self, _cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_fold_that_panics_when_a_failed_run_is_judged_is_an_error_not_a_panic() {
    // A run panicked on its input; at the end of the evaluation its reads
    // are answered again to tell a failure from a stale input, on the
    // caller's thread: a fold among them that panics now went up through
    // `evaluate` as a panic, where the caller is promised an `EvalError`.
    let e = Engine::new();
    let v = e.values();
    let broken = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s = v.field("s");
    e.declare_accumulator(s, Arc::new(Flaky(broken.clone())));
    let contrib = e.register(Arc::new(Contrib));
    let fp = e.register(Arc::new(FoldThenPanic));
    let br = e.register(Arc::new(Breaks(broken.clone())));
    let u = e.new_unit();
    e.push(u, contrib, vec![s.0, v.int(1)]);
    e.push(u, contrib, vec![s.0, v.int(2)]);
    e.push(u, fp, vec![s.0]);
    e.push(u, br, vec![]);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| e.evaluate(1).is_err()));
    assert_eq!(r.ok(), Some(true), "the evaluation returns an error");
}
