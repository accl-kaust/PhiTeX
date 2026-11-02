//! Definitions that settle at quiet points (`Operation::settles_at_quiet`,
//! docs/spec-settle.md): a change by such a node re-asks readers of such
//! operations at once and every other reader at the next quiet point,
//! its read checked again then.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, OpId, Operation, Stop, ValueId, Values};

/// `[name, x]`: defines `name` := x (a text node). Counts its runs.
struct Def {
    runs: AtomicU64,
}

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[name, x, from]`: a page-layer node: defines `name` := x, or, with
/// `from` a field, := (the value of `from` reaching it) + x. Settles at
/// quiet points. Counts its runs.
struct Page {
    runs: AtomicU64,
}

impl Operation for Page {
    fn name(&self) -> &'static str {
        "page"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let a = cx.args().to_vec();
        let mut x = v.as_int(a[1]).unwrap_or(0);
        if a.len() > 2 && a[2] != ValueId::UNIT {
            let from = cx.read(Field(a[2]))?;
            x += from.and_then(|f| v.as_int(f)).unwrap_or(0);
        }
        let out = v.int(x);
        cx.define(Field(a[0]), out);
        Ok(out)
    }

    fn settles_at_quiet(&self) -> bool {
        true
    }
}

/// `[f, g]`: a text node reading `f` and, with `g` a field, `g`; outputs
/// `f`'s value. Counts its runs.
struct Reader {
    runs: AtomicU64,
}

impl Operation for Reader {
    fn name(&self) -> &'static str {
        "reader"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let a = cx.args().to_vec();
        let x = cx.read(Field(a[0]))?;
        if a.len() > 1 && a[1] != ValueId::UNIT {
            let _ = cx.read(Field(a[1]))?;
        }
        Ok(x.unwrap_or(ValueId::UNIT))
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

struct Ops {
    e: Engine,
    def: Arc<Def>,
    page: Arc<Page>,
    reader: Arc<Reader>,
    def_op: OpId,
    page_op: OpId,
    reader_op: OpId,
}

fn setup() -> Ops {
    let e = Engine::new();
    let def = Arc::new(Def {
        runs: AtomicU64::new(0),
    });
    let page = Arc::new(Page {
        runs: AtomicU64::new(0),
    });
    let reader = Arc::new(Reader {
        runs: AtomicU64::new(0),
    });
    let def_op = e.register(def.clone());
    let page_op = e.register(page.clone());
    let reader_op = e.register(reader.clone());
    Ops {
        e,
        def,
        page,
        reader,
        def_op,
        page_op,
        reader_op,
    }
}

fn int(e: &Engine, n: phitex_defuse::NodeId) -> Option<i64> {
    e.output(n).and_then(|x| e.values().as_int(x))
}

/// Line: P1 (page: pn := 1), T1 (text: reads pn), P2 (page: pn2 := pn +
/// 1), T2 (text: reads pn2). P1's text is edited to pn := 5. P2, a page
/// node, is re-asked at once; T1 and T2 are held to the quiet point and
/// run there, once each: two held, two fired, one pass.
#[test]
fn text_readers_of_a_page_value_run_once_at_the_quiet_point() {
    let o = setup();
    let (e, v) = (&o.e, o.e.values());
    let u = e.new_unit();
    let (pn, pn2) = (v.field("pn"), v.field("pn2"));
    let p1 = e.push(u, o.page_op, vec![pn.0, v.int(1), ValueId::UNIT]);
    let t1 = e.push(u, o.reader_op, vec![pn.0]);
    let _p2 = e.push(u, o.page_op, vec![pn2.0, v.int(1), pn.0]);
    let t2 = e.push(u, o.reader_op, vec![pn2.0]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!((int(e, t1), int(e, t2)), (Some(1), Some(2)), "{rep:?}");
    assert_eq!(rep.held_fired, 0, "nothing held on a first run: {rep:?}");
    let (pr, rr) = (o.page.runs.load(Ordering::SeqCst), o.reader.runs.load(Ordering::SeqCst));
    e.replace(p1, o.page_op, vec![pn.0, v.int(5), ValueId::UNIT]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!((int(e, t1), int(e, t2)), (Some(5), Some(6)), "{rep:?}");
    assert_eq!(
        o.page.runs.load(Ordering::SeqCst),
        pr + 2,
        "P1 edited, P2 re-asked at once: {rep:?}"
    );
    assert_eq!(
        o.reader.runs.load(Ordering::SeqCst),
        rr + 2,
        "T1 and T2 once each: {rep:?}"
    );
    assert_eq!(rep.held_deferred, 2, "{rep:?}");
    assert_eq!(rep.held_fired, 2, "{rep:?}");
    assert_eq!(rep.held_passes, 1, "{rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// A held reader that ran meanwhile on the final value is not run again:
/// P (page: pn := 1), A (text: a := 1), T (text: reads pn and a). P is
/// edited to pn := 2 and A to a := 2, in that order on one thread: P's
/// commit holds T's wake; A's commit re-asks T at once (a text change),
/// T runs and reads pn = 2; at the quiet point the held wake finds T's
/// read holding and fires nothing. T ran once.
#[test]
fn a_held_reader_that_read_the_final_value_meanwhile_is_not_run_again() {
    let o = setup();
    let (e, v) = (&o.e, o.e.values());
    let u = e.new_unit();
    let (pn, a) = (v.field("pn"), v.field("a"));
    let p = e.push(u, o.page_op, vec![pn.0, v.int(1), ValueId::UNIT]);
    let an = e.push(u, o.def_op, vec![a.0, v.int(1)]);
    let t = e.push(u, o.reader_op, vec![pn.0, a.0]);
    e.evaluate(1).unwrap();
    assert_eq!(int(e, t), Some(1));
    let rr = o.reader.runs.load(Ordering::SeqCst);
    e.replace(p, o.page_op, vec![pn.0, v.int(2), ValueId::UNIT]);
    e.replace(an, o.def_op, vec![a.0, v.int(2)]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(int(e, t), Some(2), "{rep:?}");
    assert_eq!(
        o.reader.runs.load(Ordering::SeqCst),
        rr + 1,
        "T ran once, on the final values: {rep:?}"
    );
    assert_eq!(rep.held_deferred, 1, "{rep:?}");
    assert_eq!(rep.held_fired, 0, "the held wake found the read holding: {rep:?}");
    assert_eq!(rep.held_passes, 0, "{rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// A text definition read by a page node is re-asked at once, as before:
/// A (text: a := 1), P (page: pn := a + 10), T (text: reads pn). A is
/// edited: P runs in the same pass, T at the quiet point; one pass.
#[test]
fn a_page_node_reading_the_text_is_re_asked_at_once() {
    let o = setup();
    let (e, v) = (&o.e, o.e.values());
    let u = e.new_unit();
    let (pn, a) = (v.field("pn"), v.field("a"));
    let an = e.push(u, o.def_op, vec![a.0, v.int(1)]);
    let _p = e.push(u, o.page_op, vec![pn.0, v.int(10), a.0]);
    let t = e.push(u, o.reader_op, vec![pn.0]);
    e.evaluate(1).unwrap();
    assert_eq!(int(e, t), Some(11));
    e.replace(an, o.def_op, vec![a.0, v.int(3)]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(int(e, t), Some(13), "{rep:?}");
    assert_eq!(rep.held_deferred, 1, "{rep:?}");
    assert_eq!(rep.held_fired, 1, "{rep:?}");
    assert_eq!(rep.held_passes, 1, "{rep:?}");
    assert_eq!(o.def.runs.load(Ordering::SeqCst), 2);
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// Four workers, the first line's shape repeated over forty page nodes
/// and readers, thirty edits: every reader ends on the right value, no
/// wake is lost.
#[test]
fn held_wakes_are_never_lost_on_many_workers() {
    let o = setup();
    let (e, v) = (&o.e, o.e.values());
    let u = e.new_unit();
    let n = 40i64;
    let mut ps = Vec::new();
    let mut ts = Vec::new();
    for i in 0..n {
        let f = v.field(&format!("pn{i}"));
        ps.push(e.push(u, o.page_op, vec![f.0, v.int(i), ValueId::UNIT]));
        ts.push(e.push(u, o.reader_op, vec![f.0]));
    }
    e.evaluate(4).unwrap();
    for round in 1..30 {
        for (i, &p) in ps.iter().enumerate() {
            let f = v.field(&format!("pn{i}"));
            e.replace(p, o.page_op, vec![f.0, v.int(round * 100 + i as i64), ValueId::UNIT]);
        }
        let rep = e.evaluate(4).unwrap();
        for (i, &t) in ts.iter().enumerate() {
            assert_eq!(
                int(e, t),
                Some(round * 100 + i as i64),
                "round {round} reader {i}: {rep:?}"
            );
        }
        assert!(
            e.check_invariants().is_empty(),
            "round {round}: {:?}",
            e.check_invariants()
        );
    }
}

/// `[at, x]`: a page-layer node whose fold stands at node `at` (its raw
/// id): it sets the layer's frontier there (`Ctx::set_frontier`), as the
/// page loop does at a hole. Settles at quiet points.
struct Front;

impl Operation for Front {
    fn name(&self) -> &'static str {
        "front"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let a = cx.args().to_vec();
        let at = cx.values().as_int(a[0]).and_then(|x| u64::try_from(x).ok());
        cx.set_frontier(at.map(phitex_defuse::NodeId::from_raw));
        Ok(a[1])
    }

    fn settles_at_quiet(&self) -> bool {
        true
    }
}

/// A text reader past the layer's frontier, re-asked by a page value,
/// is not held to the quiet point: it waits at the frontier
/// (`Ctx::hold_at_frontier`) and runs once the layer is quiet, in the
/// same pass, on what the pages before it left. Line: F (sets the
/// frontier at A), A (text), P (page: pn := 1), T (text: reads pn). F
/// and P edited (pn := 2): F stands the frontier at A again, P's change
/// reaches T past it. Held, T ran again only at the next quiet point,
/// and a routine after it read what that run left one pass later: a
/// chain through every page, one page a pass (beautybook-en at -j4,
/// "the page layout did not settle after 12 passes", accl 22609).
#[test]
fn a_text_reader_past_the_frontier_waits_there_and_is_not_held() {
    let o = setup();
    let (e, v) = (&o.e, o.e.values());
    let front = e.register(Arc::new(Front));
    let u = e.new_unit();
    let (pn, a) = (v.field("pn"), v.field("a"));
    let f = e.push(u, front, vec![v.int(-1), v.int(0)]);
    let an = e.push(u, o.def_op, vec![a.0, v.int(1)]);
    let p = e.push(u, o.page_op, vec![pn.0, v.int(1), ValueId::UNIT]);
    let t = e.push(u, o.reader_op, vec![pn.0]);
    e.evaluate(1).unwrap();
    assert_eq!(int(e, t), Some(1));
    let at = i64::try_from(an.raw()).expect("an id");
    e.replace(f, front, vec![v.int(at), v.int(1)]);
    e.replace(p, o.page_op, vec![pn.0, v.int(2), ValueId::UNIT]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(int(e, t), Some(2), "{rep:?}");
    assert_eq!(rep.held_deferred, 0, "T past the frontier is not held: {rep:?}");
    assert_eq!(rep.held_passes, 0, "no pass: {rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// `[f, x]`: a page-layer node that predicts `f` := x and whose run
/// panics. Settles at quiet points.
struct PanicPage;

impl Operation for PanicPage {
    fn name(&self) -> &'static str {
        "panic-page"
    }

    fn run(&self, _: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        panic!("a routine that fails on inputs that hold");
    }

    fn predicted_defs(&self, args: &[ValueId], _: &Values) -> Option<Vec<(Field, ValueId)>> {
        Some(vec![(Field(args[0]), args[1])])
    }

    fn settles_at_quiet(&self) -> bool {
        true
    }
}

/// A text reader of a page node's prediction waits for its commit
/// (`Ctx::wait_for_settling`), but not for one whose run panicked on
/// inputs that hold, held for the evaluation's decision: no commit of it
/// comes, and the reader, let go at each quiet point, waited again each
/// time, and the evaluation never ended (latex_minutes_Protokol: its
/// readers ran 80 000 times each in 150 s, accl 22791). Here the
/// evaluation ends, with the panic.
#[test]
fn a_reader_does_not_wait_for_a_page_node_whose_run_panicked() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let o = setup();
        let (e, v) = (&o.e, o.e.values());
        let pp = e.register(Arc::new(PanicPage));
        let u = e.new_unit();
        let pn = v.field("pn");
        e.push(u, pp, vec![pn.0, v.int(1)]);
        e.push(u, o.reader_op, vec![pn.0]);
        let r = e.evaluate(1).map(|_| ()).map_err(|x| x.to_string());
        let _ = tx.send(r);
    });
    let r = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the evaluation ends");
    let err = r.expect_err("the panic is the evaluation's failure");
    assert!(err.contains("panicked"), "{err}");
}
