//! Loop-back: a read of the last definition on the line (`At::Final`),
//! re-asked as that definition lands or moves, capped when it never
//! settles. No rounds, no declarations: the same four rules as every
//! other read.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{At, Ctx, Engine, Field, FieldAt, Limits, NodeId, OpId, Operation, Report, Stop, ValueId, Values};

/// `[f, v]`: defines f = v (a label, late in the document).
struct Label;

impl Operation for Label {
    fn name(&self) -> &'static str {
        "label"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, v) = (Field(cx.args()[0]), cx.args()[1]);
        cx.define(f, v);
        Ok(v)
    }
}

/// `[f, v]`: as `Label`, predicting its own definition (a record of the
/// last build).
struct PredictedLabel;

impl Operation for PredictedLabel {
    fn name(&self) -> &'static str {
        "predicted_label"
    }

    fn predicted_defs(&self, args: &[ValueId], _: &Values) -> Option<Vec<(Field, ValueId)>> {
        Some(vec![(Field(args[0]), args[1])])
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, v) = (Field(cx.args()[0]), cx.args()[1]);
        cx.define(f, v);
        Ok(v)
    }
}

/// `[f]`: outputs the last definition of f on the line (a reference at
/// the front of the document to a label at its back); UNIT if none.
struct Ref(AtomicU64);

impl Operation for Ref {
    fn name(&self) -> &'static str {
        "ref"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let f = Field(cx.args()[0]);
        Ok(match cx.read_at(f, At::Final)? {
            FieldAt::Def(v, _) => v,
            FieldAt::Unset | FieldAt::Gone => ValueId::UNIT,
        })
    }
}

/// `[f, s]`: reads the last definition of f and contributes it to s (a
/// reference whose typeset width depends on the label's value).
struct RefContrib;

impl Operation for RefContrib {
    fn name(&self) -> &'static str {
        "ref_contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, s) = (Field(cx.args()[0]), Field(cx.args()[1]));
        let x = match cx.read_at(f, At::Final)? {
            FieldAt::Def(v, _) => v,
            FieldAt::Unset | FieldAt::Gone => cx.values().int(0),
        };
        cx.contribute(s, x);
        Ok(x)
    }
}

/// `[f, s]`: defines f as the number of contributions to s before it (a
/// page number that depends on what came before).
struct LabelCount;

impl Operation for LabelCount {
    fn name(&self) -> &'static str {
        "label_count"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, s) = (Field(cx.args()[0]), Field(cx.args()[1]));
        let fold = cx.fold(s, None, None)?;
        let n = cx.values().seq_len(fold);
        let x = cx.values().int(n as i64);
        cx.define(f, x);
        Ok(x)
    }
}

/// `[f, s]`: reads the last definition of f; contributes one item to s
/// only when f is unset or 0 (a reference that is wide when the label is
/// missing and narrow when it is there: the classic oscillation when
/// the label's value is the count of what came before).
struct RefFlip;

impl Operation for RefFlip {
    fn name(&self) -> &'static str {
        "ref_flip"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, s) = (Field(cx.args()[0]), Field(cx.args()[1]));
        let x = match cx.read_at(f, At::Final)? {
            FieldAt::Def(v, _) => cx.values().as_int(v).unwrap_or(0),
            FieldAt::Unset | FieldAt::Gone => 0,
        };
        if x == 0 {
            cx.contribute(s, cx.values().int(1));
        }
        Ok(cx.values().int(x))
    }
}

struct Ops {
    label: OpId,
    predicted_label: OpId,
    r: OpId,
    ref_runs: Arc<Ref>,
    ref_contrib: OpId,
    label_count: OpId,
    ref_flip: OpId,
}

fn engine() -> (Engine, Ops) {
    let e = Engine::new();
    let ref_runs = Arc::new(Ref(AtomicU64::new(0)));
    let ops = Ops {
        label: e.register(Arc::new(Label)),
        predicted_label: e.register(Arc::new(PredictedLabel)),
        r: e.register(ref_runs.clone()),
        ref_runs,
        ref_contrib: e.register(Arc::new(RefContrib)),
        label_count: e.register(Arc::new(LabelCount)),
        ref_flip: e.register(Arc::new(RefFlip)),
    };
    (e, ops)
}

fn int(e: &Engine, n: NodeId) -> i64 {
    e.values().as_int(e.output(n).expect("ran")).expect("int")
}

#[test]
fn a_final_read_sees_the_last_definition() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    let r = e.push(u, o.r, vec![x]);
    e.push(u, o.label, vec![x, v.int(3)]);
    e.push(u, o.label, vec![x, v.int(7)]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(int(&e, r), 7, "the last definition, not the first");
    assert!(
        o.ref_runs.0.load(Ordering::Relaxed) <= 2,
        "once on nothing, once on the value"
    );
    assert!(rep.capped.is_empty());
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_final_read_is_asked_again_when_the_label_moves() {
    let (e, o) = engine();
    let v = e.values();
    let (x, y) = (v.field("x").0, v.field("y").0);
    let u = e.new_unit();
    let r = e.push(u, o.r, vec![x]);
    let ry = e.push(u, o.r, vec![y]);
    let l = e.push(u, o.label, vec![x, v.int(7)]);
    e.push(u, o.label, vec![y, v.int(1)]);
    e.evaluate(2).unwrap();
    assert_eq!(int(&e, r), 7);

    e.replace(l, o.label, vec![x, v.int(8)]);
    let rep = e.evaluate(2).unwrap();
    assert_eq!(int(&e, r), 8);
    assert_eq!(int(&e, ry), 1);
    assert_eq!(rep.nodes_run, 2, "the label and its one reader: {rep:?}");
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_predicted_label_runs_its_reader_once() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    let r = e.push(u, o.r, vec![x]);
    e.push(u, o.predicted_label, vec![x, v.int(7)]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(int(&e, r), 7);
    assert_eq!(
        o.ref_runs.0.load(Ordering::Relaxed),
        1,
        "the prediction was right: no rerun"
    );
    assert!(rep.predictions_confirmed >= 1);
}

#[test]
fn a_loop_through_a_fold_converges() {
    // The reference at the front contributes what it read; the label at
    // the back is the count of contributions before it. Cold: the
    // reference contributes 0, the label becomes 1, the reference is
    // re-asked, contributes 1, the count is still 1: settled in two runs.
    let (e, o) = engine();
    let v = e.values();
    let (x, s) = (v.field("x").0, v.field("s").0);
    let u = e.new_unit();
    let r = e.push(u, o.ref_contrib, vec![x, s]);
    let l = e.push(u, o.label_count, vec![x, s]);
    let rep = e.evaluate(2).unwrap();
    assert_eq!(int(&e, l), 1);
    assert_eq!(int(&e, r), 1);
    assert!(rep.capped.is_empty());
    assert!(rep.runs <= 5, "{rep:?}");
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_loop_that_never_settles_is_capped() {
    // The reference contributes an item only when the label is 0; the
    // label is the count of items before it: 0 -> 1 -> 0 -> 1...
    let (e, o) = engine();
    e.set_limits(Limits {
        final_reruns: 6,
        ..Limits::default()
    });
    let v = e.values();
    let (x, s) = (v.field("x").0, v.field("s").0);
    let u = e.new_unit();
    let r = e.push(u, o.ref_flip, vec![x, s]);
    let l = e.push(u, o.label_count, vec![x, s]);
    let t = std::time::Instant::now();
    let rep = e.evaluate(2).unwrap();
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(rep.capped, vec![r], "the reference is the capped reader: {rep:?}");
    assert!(Report::by_op(&rep.runs_by_op, "ref_flip") <= 8, "{rep:?}");
    assert!(
        e.output(r).is_some() && e.output(l).is_some(),
        "both keep their last answer"
    );
}

#[test]
fn final_reads_reach_across_units() {
    let (e, o) = engine();
    let v = e.values();
    let x = v.field("x").0;
    let u1 = e.new_unit();
    let r = e.push(u1, o.r, vec![x]);
    let u2 = e.new_unit();
    e.push(u2, o.label, vec![x, v.int(5)]);
    e.evaluate(2).unwrap();
    assert_eq!(int(&e, r), 5);
}

#[test]
fn a_thousand_references_cost_a_thousand_re_asks_not_a_square() {
    let (e, o) = engine();
    let v = e.values();
    let u = e.new_unit();
    let mut refs = Vec::new();
    for i in 0..1000 {
        let x = v.field(&format!("x{i}")).0;
        refs.push((e.push(u, o.r, vec![x]), i));
    }
    for i in 0..1000 {
        let x = v.field(&format!("x{i}")).0;
        e.push(u, o.label, vec![x, v.int(i)]);
    }
    let t = std::time::Instant::now();
    let rep = e.evaluate(4).unwrap();
    assert!(t.elapsed() < std::time::Duration::from_millis(300), "{:?}", t.elapsed());
    for (r, i) in refs {
        assert_eq!(int(&e, r), i);
    }
    assert!(rep.runs <= 4000, "{rep:?}");
    assert!(e.check_invariants().is_empty());
}

/// `[f]`: reads f's last definition; where it is 1 (a file half
/// written: an open brace) it takes its next sibling, as a reader of an
/// unbalanced table of contents reads on into the text after it.
struct OpenReader;

impl Operation for OpenReader {
    fn name(&self) -> &'static str {
        "open_reader"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = match cx.read_at(Field(cx.args()[0]), At::Final)? {
            FieldAt::Def(x, _) => x,
            _ => ValueId::UNIT,
        };
        if cx.values().as_int(x) == Some(1) {
            let _ = cx.following(0);
            cx.claim(1);
        }
        Ok(x)
    }
}

/// `[f, v, ms]`: spins `ms` milliseconds, then defines f = v.
struct SlowLabel;

impl Operation for SlowLabel {
    fn name(&self) -> &'static str {
        "slow_label"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ms = cx.values().as_int(cx.args()[2]).unwrap() as u64;
        let t = std::time::Instant::now();
        while t.elapsed() < std::time::Duration::from_millis(ms) {
            std::hint::spin_loop();
        }
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

#[test]
fn a_half_written_file_never_leads_to_another_fixed_point() {
    // A writes the file's first half (f = 1) at once, B the whole (f = 2)
    // after a long run; R between them reads the file and, on the half,
    // takes B, which then never writes: a fixed point of its own (R reads
    // 1, B taken, the last definition 1). TeX's passes read whole files
    // only (nothing, then 2): R ends on 2 and B stands, at any threads.
    let threads = std::thread::available_parallelism().map_or(1, usize::from).clamp(2, 8);
    for j in [1, threads] {
        let (e, o) = engine();
        let open = e.register(Arc::new(OpenReader));
        let slow = e.register(Arc::new(SlowLabel));
        let v = e.values();
        let x = v.field("x").0;
        let u = e.new_unit();
        e.push(u, o.label, vec![x, v.int(1)]);
        let r = e.push(u, open, vec![x]);
        let b = e.push(u, slow, vec![x, v.int(2), v.int(40)]);
        let rep = e.evaluate(j).unwrap();
        assert_eq!(int(&e, r), 2, "-j{j}: {rep:?}");
        assert_eq!(e.node_info(b).unwrap().state, phitex_defuse::NodeState::Done, "-j{j}");
        assert_eq!(rep.passes, 2, "-j{j}: {rep:?}");
        assert!(e.check_invariants().is_empty());
    }
}
