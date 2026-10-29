//! Placements relocated, not remade (docs/spec-continuations.md §2.3): a
//! placement matched by (op, args) whose place moved keeps its node, its
//! record and its own placements; it runs again at the new place, and
//! the readers at the old place are re-asked.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, NodeId, Operation, Stop, ValueId, Values};

/// `[name, x]`: defines `name` := x.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[after_raw, key]`: places `Mark [key]` after the node `after_raw`
/// (a raw NodeId as an int) with `follow_after`; its text is edited to
/// move the place. Counts its runs.
struct Placer {
    runs: AtomicU64,
    mark: std::sync::Mutex<Option<phitex_defuse::OpId>>,
}

impl Operation for Placer {
    fn name(&self) -> &'static str {
        "placer"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let after = NodeId::from_raw(v.as_int(cx.args()[0]).unwrap_or(0).unsigned_abs());
        let mark = self.mark.lock().unwrap().expect("the mark op");
        cx.follow_after(after, mark, vec![cx.args()[1]]);
        Ok(ValueId::UNIT)
    }
}

/// `[key]`: defines `m` := key and places a child `Def [c, key]`. Counts
/// its runs.
struct Mark {
    runs: AtomicU64,
    def: std::sync::Mutex<Option<phitex_defuse::OpId>>,
}

impl Operation for Mark {
    fn name(&self) -> &'static str {
        "mark"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        cx.define(v.field("m"), cx.args()[0]);
        let def = self.def.lock().unwrap().expect("the def op");
        cx.child(def, vec![v.field("c").0, cx.args()[0]]);
        Ok(cx.args()[0])
    }
}

/// `[]`: reads `m` and outputs it. Counts its runs.
struct Reader {
    runs: AtomicU64,
}

impl Operation for Reader {
    fn name(&self) -> &'static str {
        "reader"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let x = cx.read(v.field("m"))?;
        Ok(x.unwrap_or(ValueId::UNIT))
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

fn setup() -> (
    Engine,
    Arc<Placer>,
    Arc<Mark>,
    Arc<Reader>,
    phitex_defuse::OpId,
    phitex_defuse::OpId,
    phitex_defuse::OpId,
    phitex_defuse::OpId,
) {
    let e = Engine::new();
    let def = e.register(Arc::new(Def));
    let mk = Arc::new(Mark {
        runs: AtomicU64::new(0),
        def: std::sync::Mutex::new(Some(def)),
    });
    let mark = e.register(mk.clone());
    let pl = Arc::new(Placer {
        runs: AtomicU64::new(0),
        mark: std::sync::Mutex::new(Some(mark)),
    });
    let placer = e.register(pl.clone());
    let rd = Arc::new(Reader {
        runs: AtomicU64::new(0),
    });
    let reader = e.register(rd.clone());
    (e, pl, mk, rd, def, mark, placer, reader)
}

/// Line: A (def), B (def), the placer placing X=Mark after A, a reader R
/// after B. Then the placer's text is edited to place X after B: X is the
/// same node, it ran once more, its definition stands after B, R was
/// re-asked once, and X's child is the same node, after X.
#[test]
fn a_moved_placement_keeps_its_node_and_its_placements() {
    let (e, pl, mk, rd, def, _mark, placer, reader) = setup();
    let v = e.values();
    let u = e.new_unit();
    let a = e.push(u, def, vec![v.field("a").0, v.int(1)]);
    let b = e.push(u, def, vec![v.field("b").0, v.int(2)]);
    let p = e.push(u, placer, vec![v.int(i64::try_from(a.raw()).unwrap()), v.int(7)]);
    let r = e.push(u, reader, vec![]);
    let rep = e.evaluate(2).unwrap();
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(7), "{rep:?}");
    let x1 = e.placed_by(p);
    assert_eq!(x1.len(), 1, "one placement: {rep:?}");
    let child1 = e.placed_by(x1[0]);
    assert_eq!(child1.len(), 1);
    let (mk_runs, rd_runs) = (mk.runs.load(Ordering::SeqCst), rd.runs.load(Ordering::SeqCst));
    // the placer now places X after B
    e.replace(p, placer, vec![v.int(i64::try_from(b.raw()).unwrap()), v.int(7)]);
    let rep = e.evaluate(2).unwrap();
    let x2 = e.placed_by(p);
    assert_eq!(x2, x1, "the same node, relocated: {rep:?}");
    assert_eq!(e.placed_by(x2[0]), child1, "its own placement follows it: {rep:?}");
    assert_eq!(
        mk.runs.load(Ordering::SeqCst),
        mk_runs + 1,
        "X ran once more at its new place: {rep:?}"
    );
    assert!(e.before(b, x2[0]), "X stands after B now");
    assert_eq!(rep.placements_moved, 1, "{rep:?}");
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(7), "{rep:?}");
    assert!(
        rd.runs.load(Ordering::SeqCst) <= rd_runs + 1,
        "the reader re-asked at most once: {rep:?}"
    );
    assert_eq!(pl.runs.load(Ordering::SeqCst), 2);
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// A reader between the old and the new place: it read X's `m` before the
/// move and answers none after it (nothing defines `m` before it now).
#[test]
fn a_relocation_re_asks_the_readers_at_the_old_place() {
    let (e, _pl, _mk, rd, def, _mark, placer, reader) = setup();
    let v = e.values();
    let u = e.new_unit();
    let a = e.push(u, def, vec![v.field("a").0, v.int(1)]);
    let p = e.push(u, placer, vec![v.int(i64::try_from(a.raw()).unwrap()), v.int(7)]);
    let r1 = e.push(u, reader, vec![]);
    let b = e.push(u, def, vec![v.field("b").0, v.int(2)]);
    let r2 = e.push(u, reader, vec![]);
    let rep = e.evaluate(2).unwrap();
    assert_eq!(e.output(r1).and_then(|x| v.as_int(x)), Some(7), "{rep:?}");
    assert_eq!(e.output(r2).and_then(|x| v.as_int(x)), Some(7), "{rep:?}");
    let before = rd.runs.load(Ordering::SeqCst);
    e.replace(p, placer, vec![v.int(i64::try_from(b.raw()).unwrap()), v.int(7)]);
    let rep = e.evaluate(2).unwrap();
    assert_eq!(e.output(r1), Some(ValueId::UNIT), "r1 stands before X now: {rep:?}");
    assert_eq!(e.output(r2).and_then(|x| v.as_int(x)), Some(7), "{rep:?}");
    assert_eq!(rd.runs.load(Ordering::SeqCst), before + 1, "only r1 re-asked: {rep:?}");
    assert_eq!(rep.placements_moved, 1, "{rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}
