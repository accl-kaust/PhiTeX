//! A node woken for a read whose value moved back before it ran: every
//! read of its last run gives what it gave, so it does not run, and what
//! that run made stands (`Ctx::last_run_stands`). Grail's last pages'
//! routines ran 3.8 s of hyperxmp at -j8 on such wakes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{At, Ctx, Engine, FieldAt, OpId, Operation, Stop, ValueId};

/// `[x]`: defines `y` := x.
struct Late;

impl Operation for Late {
    fn name(&self) -> &'static str {
        "late"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        cx.define(v.field("y"), cx.args()[0]);
        Ok(ValueId::UNIT)
    }
}

/// `[t]`: reads `y` at the unit's end, defines `x` := t + y (0 while `y`
/// has no definition).
struct Early;

impl Operation for Early {
    fn name(&self) -> &'static str {
        "early"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let t = v.as_int(cx.args()[0]).unwrap_or(0);
        let y = match cx.read_at(v.field("y"), At::End(cx.unit()))? {
            FieldAt::Def(x, _) => v.as_int(x).unwrap_or(0),
            _ => 0,
        };
        cx.define(v.field("x"), v.int(t + y));
        Ok(ValueId::UNIT)
    }
}

/// `[]`: reads `x`, outputs it, places a `Kid`; counts its runs.
struct Reader {
    runs: AtomicU64,
    kid: OpId,
}

impl Operation for Reader {
    fn name(&self) -> &'static str {
        "reader"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let x = cx.read(v.field("x"))?.unwrap_or(ValueId::UNIT);
        cx.child(self.kid, vec![x]);
        Ok(x)
    }
}

/// `[x]`: defines `z` := x.
struct Kid;

impl Operation for Kid {
    fn name(&self) -> &'static str {
        "kid"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        cx.define(v.field("z"), cx.args()[0]);
        Ok(cx.args()[0])
    }
}

/// `[]`: reads `z`.
struct Last;

impl Operation for Last {
    fn name(&self) -> &'static str {
        "last"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        Ok(cx.read(v.field("z"))?.unwrap_or(ValueId::UNIT))
    }
}

/// Early (t 1), Late (y 1), the reader, what reads its child: x = 2. Early
/// edited to 2 and Late to 0: Early runs first on the old `y`, x = 3 and
/// the reader is asked for; Late's commit asks Early again, x = 2, before
/// the reader's turn at -j1.
#[test]
fn a_value_moved_back_before_its_reader_ran_runs_nothing() {
    let e = Engine::new();
    let v = e.values();
    let late = e.register(Arc::new(Late));
    let early = e.register(Arc::new(Early));
    let kid = e.register(Arc::new(Kid));
    let reader_op = Arc::new(Reader {
        runs: AtomicU64::new(0),
        kid,
    });
    let reader = e.register(reader_op.clone());
    let last = e.register(Arc::new(Last));
    let u = e.new_unit();
    let a = e.push(u, early, vec![v.int(1)]);
    let b = e.push(u, late, vec![v.int(1)]);
    let r = e.push(u, reader, vec![]);
    let l = e.push(u, last, vec![]);
    e.evaluate(1).unwrap();
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(2));
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(2));
    let k = e.placed_by(r);

    let before = reader_op.runs.load(Ordering::SeqCst);
    e.replace(a, early, vec![v.int(2)]);
    e.replace(b, late, vec![v.int(0)]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(
        reader_op.runs.load(Ordering::SeqCst),
        before,
        "nothing to run for: {rep:?}"
    );
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(2), "{rep:?}");
    assert_eq!(e.placed_by(r), k, "its child stands: {rep:?}");
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(2), "{rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());

    // (a value that did move still runs it, and what it reads on)
    e.replace(a, early, vec![v.int(5)]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(reader_op.runs.load(Ordering::SeqCst), before + 1, "{rep:?}");
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(5), "{rep:?}");
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(5), "{rep:?}");

    // (and any order of it at more threads comes to the same)
    for t in [3, 4, 3, 7] {
        e.replace(a, early, vec![v.int(t)]);
        e.replace(b, late, vec![v.int(7 - t)]);
        e.evaluate(4).unwrap();
        assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(7));
        assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(7));
    }
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// `[t]`: sleeps `t` ms, defines `x` := t.
struct SlowDef;

impl Operation for SlowDef {
    fn name(&self) -> &'static str {
        "slowdef"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let t = v.as_int(cx.args()[0]).unwrap_or(0);
        std::thread::sleep(std::time::Duration::from_millis(t.unsigned_abs()));
        cx.define(v.field("x"), cx.args()[0]);
        Ok(ValueId::UNIT)
    }
}

/// `[y]`: reads `x`, takes 120 ms; outputs x + y; counts its runs.
struct Long {
    runs: AtomicU64,
}

impl Operation for Long {
    fn name(&self) -> &'static str {
        "long"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let x = cx.read(v.field("x"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
        std::thread::sleep(std::time::Duration::from_millis(120));
        Ok(v.int(x + v.as_int(cx.args()[0]).unwrap_or(0)))
    }
}

/// A long run asked for together with a node before it: on free threads it
/// waits for that node, and runs once, on what that node makes.
#[test]
fn a_long_run_waits_for_the_nodes_before_it() {
    let e = Engine::new();
    let v = e.values();
    let slow = e.register(Arc::new(SlowDef));
    let long_op = Arc::new(Long {
        runs: AtomicU64::new(0),
    });
    let long = e.register(long_op.clone());
    let u = e.new_unit();
    let a = e.push(u, slow, vec![v.int(1)]);
    let b = e.push(u, long, vec![v.int(10)]);
    e.evaluate(4).unwrap();
    assert_eq!(e.output(b).and_then(|x| v.as_int(x)), Some(11));
    let before = long_op.runs.load(Ordering::SeqCst);
    e.replace(a, slow, vec![v.int(50)]);
    e.replace(b, long, vec![v.int(20)]);
    let rep = e.evaluate(4).unwrap();
    assert_eq!(e.output(b).and_then(|x| v.as_int(x)), Some(70), "{rep:?}");
    assert_eq!(long_op.runs.load(Ordering::SeqCst), before + 1, "{rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}
