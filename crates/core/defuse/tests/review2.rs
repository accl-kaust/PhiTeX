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

/// `[f, v]`: defines f = v (a label).
struct Label;

impl Operation for Label {
    fn name(&self) -> &'static str {
        "label"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, x) = (phitex_defuse::Field(cx.args()[0]), cx.args()[1]);
        cx.define(f, x);
        Ok(x)
    }
}

/// `[f]`: the last definition of f on the line (a reference to a label
/// after it).
struct Ref;

impl Operation for Ref {
    fn name(&self) -> &'static str {
        "ref"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(match cx.read_at(phitex_defuse::Field(cx.args()[0]), phitex_defuse::At::Final)? {
            phitex_defuse::FieldAt::Def(x, _) => x,
            phitex_defuse::FieldAt::Unset | phitex_defuse::FieldAt::Gone => ValueId::UNIT,
        })
    }
}

/// Edits the label's value to 1, 2, .. `n`, one each time it is asked
/// (a user typing while the build runs).
struct Typing {
    label: NodeId,
    op: OpId,
    f: phitex_defuse::Field,
    k: i64,
    n: i64,
}

impl phitex_defuse::EditSource for Typing {
    fn absorb(&mut self, eng: &Engine) -> Option<u64> {
        if self.k == self.n {
            return None;
        }
        self.k += 1;
        let v = eng.values();
        eng.replace(self.label, self.op, vec![self.f.0, v.int(self.k)]);
        Some(self.k.unsigned_abs())
    }
}

#[test]
fn edits_taken_during_an_evaluation_each_get_their_own_loop_back_passes() {
    // A reference to a label after it, the label edited fifteen times
    // while one evaluation runs: each edit is a new document whose
    // reference settles in one more pass. The passes were counted over
    // the whole evaluation, so after the tenth edit the reference was
    // capped and kept the tenth value, the document never converging as
    // far as the report says.
    let e = Engine::new();
    let v = e.values();
    let label = e.register(Arc::new(Label));
    let rf = e.register(Arc::new(Ref));
    let f = v.field("lab");
    let u = e.new_unit();
    let r = e.push(u, rf, vec![f.0]);
    let l = e.push(u, label, vec![f.0, v.int(0)]);
    let mut src = Typing { label: l, op: label, f, k: 0, n: 15 };
    let rep = e.evaluate_absorbing(2, &mut src).unwrap();
    assert_eq!(rep.version, 15);
    assert!(rep.capped.is_empty(), "nothing is capped: {:?}", rep.capped);
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(15), "{rep:?}");
    assert!(e.check_invariants().is_empty());
}

/// Removes the label the first time it is asked (a user deleting the
/// `\label` while the build runs).
struct DeleteLabel(NodeId, bool);

impl phitex_defuse::EditSource for DeleteLabel {
    fn absorb(&mut self, eng: &Engine) -> Option<u64> {
        if std::mem::replace(&mut self.1, true) {
            return None;
        }
        eng.remove(self.0);
        Some(1)
    }
}

#[test]
fn an_edit_taken_at_the_last_quiet_point_is_published_before_the_evaluation_ends() {
    // The edit is taken at the quiet point that would end the evaluation:
    // it removes the label a reference before it reads back, which queues
    // no node but changes the last definition of the label. The evaluation
    // ended there, with nothing queued, and the reference kept the removed
    // label's value: the report's version was the edited document's, its
    // output was not.
    let e = Engine::new();
    let v = e.values();
    let label = e.register(Arc::new(Label));
    let rf = e.register(Arc::new(Ref));
    let f = v.field("lab");
    let u = e.new_unit();
    let r = e.push(u, rf, vec![f.0]);
    let l = e.push(u, label, vec![f.0, v.int(7)]);
    let mut src = DeleteLabel(l, false);
    let rep = e.evaluate_absorbing(1, &mut src).unwrap();
    assert_eq!(rep.version, 1);
    assert_eq!(e.output(r), Some(ValueId::UNIT), "the reference reads no label: {rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// Gates for a run that straddles an edit taken mid-evaluation.
#[derive(Default)]
struct EditGates {
    absorbed: std::sync::atomic::AtomicBool,
    marked_done: std::sync::atomic::AtomicBool,
}

fn wait_gate(flag: &std::sync::atomic::AtomicBool, then_ms: u64) {
    let t = std::time::Instant::now();
    while !flag.load(std::sync::atomic::Ordering::Acquire) && t.elapsed() < std::time::Duration::from_secs(5) {
        std::hint::spin_loop();
    }
    let t = std::time::Instant::now();
    while t.elapsed() < std::time::Duration::from_millis(then_ms) {
        std::hint::spin_loop();
    }
}

/// `[x, gated]`: defines "x" = x; gated, it waits for the marked node's
/// run to end first.
struct GatedDef(Arc<EditGates>);

impl Operation for GatedDef {
    fn name(&self) -> &'static str {
        "gated_def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        if cx.values().as_int(cx.args()[1]) == Some(1) {
            wait_gate(&self.0.marked_done, 30);
        }
        let x = cx.args()[0];
        cx.define(cx.values().field("x"), x);
        Ok(x)
    }
}

/// `[gated]`: reads "x"; gated, it waits for the edit before it reads and
/// raises `marked_done` at its end.
struct GatedRead(Arc<EditGates>);

impl Operation for GatedRead {
    fn name(&self) -> &'static str {
        "gated_read"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let gated = cx.values().as_int(cx.args()[0]) == Some(1);
        if gated {
            wait_gate(&self.0.absorbed, 0);
        }
        let x = cx.read(cx.values().field("x"))?.unwrap_or(ValueId::UNIT);
        if gated {
            self.0.marked_done.store(true, std::sync::atomic::Ordering::Release);
        }
        Ok(x)
    }
}

/// `[ms]`: works `ms` milliseconds.
struct Busy;

impl Operation for Busy {
    fn name(&self) -> &'static str {
        "busy"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ms = cx.values().as_int(cx.args()[0]).unwrap_or(0).unsigned_abs();
        let t = std::time::Instant::now();
        while t.elapsed() < std::time::Duration::from_millis(ms) {
            std::hint::spin_loop();
        }
        Ok(ValueId::UNIT)
    }
}

/// Edits the definer to 2 the first time it is asked; keeps what settles.
struct EditDef {
    def: NodeId,
    op: OpId,
    g: Arc<EditGates>,
    done: bool,
    seen: Vec<phitex_defuse::Settled>,
}

impl phitex_defuse::EditSource for EditDef {
    fn absorb(&mut self, eng: &Engine) -> Option<u64> {
        if std::mem::replace(&mut self.done, true) {
            return None;
        }
        let v = eng.values();
        eng.replace(self.def, self.op, vec![v.int(2), v.int(1)]);
        self.g.absorbed.store(true, std::sync::atomic::Ordering::Release);
        Some(1)
    }

    fn settled(&mut self, _eng: &Engine, items: &[phitex_defuse::Settled]) -> bool {
        self.seen.extend_from_slice(items);
        false
    }
}

#[test]
fn a_node_settled_at_a_version_has_that_version_s_output() {
    // A marked node's run began before an edit taken mid-evaluation and
    // read the old value; it committed after the edit was taken, before
    // the edited definer committed. It was reported settled at the edit's
    // version with the output of the document before it (and then again
    // with the right one): a client taking the first report of a version
    // shows a stale page as current.
    let e = Engine::new();
    let v = e.values();
    let g = Arc::new(EditGates::default());
    let def = e.register(Arc::new(GatedDef(g.clone())));
    let rd = e.register(Arc::new(GatedRead(g.clone())));
    let busy = e.register(Arc::new(Busy));
    let u = e.new_unit();
    let d = e.push(u, def, vec![v.int(1), v.int(0)]);
    let fs: Vec<NodeId> = (0..4).map(|_| e.push(u, busy, vec![v.int(3)])).collect();
    let m = e.push(u, rd, vec![v.int(0)]);
    e.evaluate(2).unwrap();
    e.mark(m);
    let _ = e.take_settled();
    for &f in &fs {
        e.replace(f, busy, vec![v.int(3)]);
    }
    e.replace(m, rd, vec![v.int(1)]);
    let mut src = EditDef { def: d, op: def, g, done: false, seen: Vec::new() };
    let rep = e.evaluate_absorbing(2, &mut src).unwrap();
    assert_eq!(e.output(m).and_then(|x| v.as_int(x)), Some(2), "{rep:?}");
    assert!(!src.seen.is_empty());
    for s in &src.seen {
        if s.version == 1 {
            assert_eq!(v.as_int(s.output), Some(2), "settled at version 1 with the old output: {:?} {rep:?}", src.seen);
        }
    }
}
