//! A node woken by another's commit runs once that node is done, not once
//! per step of it (`sched::wait_for_writer`): a value settling in steps
//! costs its reader one run after the last step.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{At, Ctx, Engine, Field, FieldAt, OpId, Operation, Stop, ValueId, Values};

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

/// `[k]`: reads `g` as the last definition in its unit, defines `f` :=
/// g + k, and with k != 0 places a child `Def [g, 2]`: its own child then
/// changes what it read, and it runs again. Counts its runs.
struct Writer {
    runs: AtomicU64,
    def: std::sync::Mutex<Option<OpId>>,
}

impl Operation for Writer {
    fn name(&self) -> &'static str {
        "writer"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let g = match cx.read_at(v.field("g"), At::End(cx.unit()))? {
            FieldAt::Def(x, _) => v.as_int(x).unwrap_or(0),
            FieldAt::Unset | FieldAt::Gone => 0,
        };
        let k = v.as_int(cx.args()[0]).unwrap_or(0);
        let f = v.int(g + k);
        cx.define(v.field("f"), f);
        if k != 0 {
            let def = self.def.lock().unwrap().expect("the def op");
            cx.child(def, vec![v.field("g").0, v.int(2)]);
        }
        Ok(f)
    }
}

/// `[]`: reads `f` and outputs it. Counts its runs.
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
        let x = cx.read(v.field("f"))?;
        Ok(x.unwrap_or(ValueId::UNIT))
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

fn setup() -> (Engine, Arc<Writer>, Arc<Reader>, OpId, OpId, OpId) {
    let e = Engine::new();
    let def = e.register(Arc::new(Def));
    let wr = Arc::new(Writer { runs: AtomicU64::new(0), def: std::sync::Mutex::new(Some(def)) });
    let writer = e.register(wr.clone());
    let rd = Arc::new(Reader { runs: AtomicU64::new(0) });
    let reader = e.register(rd.clone());
    (e, wr, rd, def, writer, reader)
}

/// Line: A defines g := 1, W reads g (the unit's last) and defines f :=
/// g + k, R reads f. W's text is edited to k = 10: W runs (f = 11), and
/// its child C := 2 is placed; C runs, and W is re-asked; R, re-asked
/// by W's commit, is taken from the deque while W is queued again: it
/// waits, W runs (f = 12), and R runs once, on 12. One thread: the deque
/// order is fixed (the child placed first, then the re-asked in order).
#[test]
fn a_reader_waits_for_its_writer_to_settle_and_runs_once() {
    let (e, wr, rd, def, writer, reader) = setup();
    let v = e.values();
    let u = e.new_unit();
    let _a = e.push(u, def, vec![v.field("g").0, v.int(1)]);
    let w = e.push(u, writer, vec![v.int(0)]);
    let r = e.push(u, reader, vec![]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(1), "{rep:?}");
    assert_eq!((wr.runs.load(Ordering::SeqCst), rd.runs.load(Ordering::SeqCst)), (1, 1));
    e.replace(w, writer, vec![v.int(10)]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(12), "{rep:?}");
    assert_eq!(wr.runs.load(Ordering::SeqCst), 3, "W ran on g = 1, then on its child's 2: {rep:?}");
    assert_eq!(rd.runs.load(Ordering::SeqCst), 2, "R ran once in the second evaluation, after W settled: {rep:?}");
    assert_eq!(rep.runs_waited, 1, "{rep:?}");
    assert_eq!(rep.waits_broken, 0, "{rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// The same line on four workers, edited back and forth thirty times:
/// every evaluation ends with the right answer and nothing left waiting
/// (a waiter is never lost: its waker's end or the quiet point releases
/// it).
#[test]
fn waiters_are_always_released_on_many_workers() {
    let (e, _wr, _rd, def, writer, reader) = setup();
    let v = e.values();
    let u = e.new_unit();
    let _a = e.push(u, def, vec![v.field("g").0, v.int(1)]);
    let w = e.push(u, writer, vec![v.int(0)]);
    let r = e.push(u, reader, vec![]);
    let rep = e.evaluate(4).unwrap();
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(1), "{rep:?}");
    for i in 0..30 {
        let k = if i % 2 == 0 { 10 } else { 20 };
        e.replace(w, writer, vec![v.int(k)]);
        let rep = e.evaluate(4).unwrap();
        assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(2 + k), "edit {i}: {rep:?}");
        assert!(e.check_invariants().is_empty(), "edit {i}: {:?}", e.check_invariants());
    }
}
