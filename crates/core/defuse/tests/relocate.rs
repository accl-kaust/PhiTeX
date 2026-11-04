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

/// The engine, the placer, mark and reader operations, and the ids of
/// def, mark, placer and reader.
type Setup = (
    Engine,
    Arc<Placer>,
    Arc<Mark>,
    Arc<Reader>,
    phitex_defuse::OpId,
    phitex_defuse::OpId,
    phitex_defuse::OpId,
    phitex_defuse::OpId,
);

fn setup() -> Setup {
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

/// `[key]`: defines `m` := key globally. Counts its runs.
struct GMark {
    runs: AtomicU64,
}

impl Operation for GMark {
    fn name(&self) -> &'static str {
        "gmark"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        cx.define_with(v.field("m"), cx.args()[0], phitex_defuse::Locality::Global);
        Ok(cx.args()[0])
    }
}

/// `[open?]`: opens one scope after itself (args `[1]`), or closes one
/// before itself (`[0]`).
struct Bracket;

impl Operation for Bracket {
    fn name(&self) -> &'static str {
        "bracket"
    }

    fn effect(&self, args: &[ValueId], v: &Values) -> phitex_defuse::Effect {
        if v.as_int(args[0]) == Some(1) {
            phitex_defuse::Effect::Open
        } else {
            phitex_defuse::Effect::Close
        }
    }

    fn run(&self, _cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

/// A node that defines `m` globally inside a scope, moved within it: a
/// reader past the scope's close sees its definition at the new place as
/// at the old, and is not run again. Its definitions were filed at the
/// new place at depth 0, seen only inside the scope, so the reader was
/// re-asked into the definition before the scope (none here), ran, and
/// ran again once the moved node had (Grail: the routine that voids a
/// float's box moved, every later routine reading the class's box ran
/// again, mc 79156).
#[test]
fn a_global_definition_moved_inside_a_scope_still_reaches_past_its_close() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let gm = Arc::new(GMark {
        runs: AtomicU64::new(0),
    });
    let gmark = e.register(gm.clone());
    let pl = Arc::new(Placer {
        runs: AtomicU64::new(0),
        mark: std::sync::Mutex::new(Some(gmark)),
    });
    let placer = e.register(pl.clone());
    let rd = Arc::new(Reader {
        runs: AtomicU64::new(0),
    });
    let reader = e.register(rd.clone());
    let bracket = e.register(Arc::new(Bracket));
    let u = e.new_unit();
    let _open = e.push(u, bracket, vec![v.int(1)]);
    let a = e.push(u, def, vec![v.field("a").0, v.int(1)]);
    let b = e.push(u, def, vec![v.field("b").0, v.int(2)]);
    let p = e.push(u, placer, vec![v.int(i64::try_from(a.raw()).unwrap()), v.int(7)]);
    let _close = e.push(u, bracket, vec![v.int(0)]);
    let r = e.push(u, reader, vec![]);
    let rep = e.evaluate(2).unwrap();
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(7), "{rep:?}");
    let x1 = e.placed_by(p);
    let (gm_runs, rd_runs) = (gm.runs.load(Ordering::SeqCst), rd.runs.load(Ordering::SeqCst));
    // the placer now places X after B, still inside the scope
    e.replace(p, placer, vec![v.int(i64::try_from(b.raw()).unwrap()), v.int(7)]);
    let rep = e.evaluate(2).unwrap();
    assert_eq!(e.placed_by(p), x1, "the same node, relocated: {rep:?}");
    assert!(e.before(b, x1[0]), "X stands after B now");
    assert_eq!(rep.placements_moved, 1, "{rep:?}");
    assert_eq!(gm.runs.load(Ordering::SeqCst), gm_runs + 1, "X ran once more: {rep:?}");
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(7), "{rep:?}");
    assert_eq!(
        rd.runs.load(Ordering::SeqCst),
        rd_runs,
        "the reader past the scope's close is not run again: {rep:?}"
    );
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// `[key]`: reads `a` and defines `m` := it. Counts its runs.
struct CopyA {
    runs: AtomicU64,
}

impl Operation for CopyA {
    fn name(&self) -> &'static str {
        "copy"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let a = cx.read(v.field("a"))?.unwrap_or(ValueId::UNIT);
        cx.define(v.field("m"), a);
        Ok(a)
    }
}

/// `[]`: reads `a`, then `m`, and outputs `m`. Counts its runs.
struct Reader2 {
    runs: AtomicU64,
}

impl Operation for Reader2 {
    fn name(&self) -> &'static str {
        "reader2"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        cx.read(v.field("a"))?;
        Ok(cx.read(v.field("m"))?.unwrap_or(ValueId::UNIT))
    }
}

/// A node queued to run (its read moved) and then moved before readers
/// of its definition runs before them, at its new place: queued at its
/// old place's key, it ran after them, and each ran twice (Grail's cold
/// build, a page's routine moved to the piece it now fires in after the
/// text before it changed: the pieces after that piece held at the
/// frontier on the routine's definitions until it ran, 331 holds in a
/// pass, mc 79156).
#[test]
fn a_queued_node_moved_before_its_readers_runs_before_them() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let cp = Arc::new(CopyA {
        runs: AtomicU64::new(0),
    });
    let copy = e.register(cp.clone());
    let pl = Arc::new(Placer {
        runs: AtomicU64::new(0),
        mark: std::sync::Mutex::new(Some(copy)),
    });
    let placer = e.register(pl.clone());
    let rd = Arc::new(Reader2 {
        runs: AtomicU64::new(0),
    });
    let reader = e.register(rd.clone());
    let u = e.new_unit();
    let a = e.push(u, def, vec![v.field("a").0, v.int(1)]);
    let p0 = e.push(u, def, vec![v.field("b").0, v.int(0)]);
    // the placer stands before the readers, placing X after `z`, after
    // them: they see none
    let p = e.push(u, placer, vec![v.int(0), v.int(7)]);
    let rs: Vec<NodeId> = (0..4).map(|_| e.push(u, reader, vec![])).collect();
    let z = e.push(u, def, vec![v.field("z").0, v.int(0)]);
    e.replace(p, placer, vec![v.int(i64::try_from(z.raw()).unwrap()), v.int(7)]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(e.output(rs[0]), Some(ValueId::UNIT), "{rep:?}");
    let x1 = e.placed_by(p);
    assert_eq!(x1.len(), 1);
    let rd_runs = rd.runs.load(Ordering::SeqCst);
    // `a` changes (X and the readers are queued to run again) and X moves
    // after `p0`, before the readers
    e.replace(a, def, vec![v.field("a").0, v.int(2)]);
    e.replace(p, placer, vec![v.int(i64::try_from(p0.raw()).unwrap()), v.int(7)]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(e.placed_by(p), x1, "the same node, relocated: {rep:?}");
    assert!(e.before(x1[0], rs[0]), "X stands before the readers now");
    for &r in &rs {
        assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(2), "{rep:?}");
    }
    assert_eq!(
        rd.runs.load(Ordering::SeqCst),
        rd_runs + rs.len() as u64,
        "each reader ran once, after X: {rep:?}"
    );
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// Line: A, B, the placer P placing X = Mark after B (X places its child
/// C), then a second placer Q placing Y = Mark after X. P is edited to
/// place X after A, before B: X moves back, and C and Y, which stand
/// under X, go with it, in their order, before B. Left after B, C stood
/// past B, and Y, placed after X and its children, after C there
/// (numerica at -j4: a page's finisher after its piece's next pages).
#[test]
fn a_placement_moved_back_takes_what_stands_under_it() {
    let (e, _pl, _mk, _rd, def, _mark, placer, _reader) = setup();
    let v = e.values();
    let u = e.new_unit();
    let raw = |n: NodeId| v.int(i64::try_from(n.raw()).unwrap());
    let a = e.push(u, def, vec![v.field("a").0, v.int(1)]);
    let b = e.push(u, def, vec![v.field("b").0, v.int(2)]);
    let p = e.push(u, placer, vec![raw(b), v.int(7)]);
    let rep = e.evaluate(2).unwrap();
    let x = e.placed_by(p);
    assert_eq!(x.len(), 1, "{rep:?}");
    let x = x[0];
    let c = e.placed_by(x);
    assert_eq!(c.len(), 1, "{rep:?}");
    let c = c[0];
    let q = e.push(u, placer, vec![raw(x), v.int(8)]);
    let rep = e.evaluate(2).unwrap();
    let y = e.placed_by(q);
    assert_eq!(y.len(), 1, "{rep:?}");
    let y = y[0];
    assert!(
        e.before(b, x) && e.before(x, c) && e.before(c, y),
        "X, C, Y after B: {rep:?}"
    );
    e.replace(p, placer, vec![raw(a), v.int(7)]);
    let rep = e.evaluate(2).unwrap();
    assert_eq!(e.placed_by(p), vec![x], "the same node, relocated: {rep:?}");
    assert!(e.before(a, x) && e.before(x, b), "X between A and B: {rep:?}");
    assert!(
        e.before(x, c) && e.before(c, b),
        "its child C with it, before B: {rep:?}"
    );
    assert!(
        e.before(c, y) && e.before(y, b),
        "Y, placed after X, with it, before B: {rep:?}"
    );
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}
