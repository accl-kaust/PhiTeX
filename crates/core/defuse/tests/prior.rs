//! A node woken for a read whose value moved and moved back: its last
//! run read the value between, the run before it the value now. That
//! run's record stands again where every read of it gives what it gave
//! (`Node::prior`, `Ctx::last_run_stands`): nothing runs, and what it
//! made is the node's again. Grail's picture after its citations, which
//! loads the bold font first in the second pass alone, ran once a pass
//! for the output of its first run.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, OpId, Operation, Stop, ValueId};

/// `[x]`: defines `x`.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        cx.define(v.field("x"), cx.args()[0]);
        Ok(ValueId::UNIT)
    }
}

/// `[]`: reads `x`, defines `y` := 10 x and contributes it to `log`,
/// outputs it; counts its runs. With `kid`, places one on it.
struct Reader {
    runs: AtomicU64,
    kid: Option<OpId>,
}

impl Operation for Reader {
    fn name(&self) -> &'static str {
        "reader"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let x = cx.read(v.field("x"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
        let y = v.int(10 * x);
        cx.define(v.field("y"), y);
        cx.contribute(v.field("log"), y);
        if let Some(k) = self.kid {
            cx.child(k, vec![y]);
        }
        Ok(y)
    }
}

/// `[x]`: outputs x.
struct Kid;

impl Operation for Kid {
    fn name(&self) -> &'static str {
        "kid"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx.args()[0])
    }
}

/// `[]`: reads `y`, and the fold of `log`.
struct Tail;

impl Operation for Tail {
    fn name(&self) -> &'static str {
        "tail"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let y = cx.read(v.field("y"))?.unwrap_or(ValueId::UNIT);
        let log = cx.fold(v.field("log"), None, Some(cx.node()))?;
        Ok(v.list(vec![y, log]))
    }
}

fn setup(
    kid: bool,
) -> (
    Engine,
    Arc<Reader>,
    OpId,
    phitex_defuse::NodeId,
    phitex_defuse::NodeId,
    phitex_defuse::NodeId,
) {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let k = e.register(Arc::new(Kid));
    let reader_op = Arc::new(Reader {
        runs: AtomicU64::new(0),
        kid: kid.then_some(k),
    });
    let reader = e.register(reader_op.clone());
    let tail = e.register(Arc::new(Tail));
    let u = e.new_unit();
    let d = e.push(u, def, vec![v.int(1)]);
    let r = e.push(u, reader, vec![]);
    let t = e.push(u, tail, vec![]);
    (e, reader_op, def, d, r, t)
}

#[test]
fn a_value_moved_and_moved_back_takes_the_run_before_the_last() {
    for threads in [1, 4] {
        let (e, op, def, d, r, t) = setup(false);
        let v = e.values();
        e.evaluate(threads).unwrap();
        let first = e.output(t);
        assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(10));
        e.replace(d, def, vec![v.int(2)]);
        e.evaluate(threads).unwrap();
        assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(20));
        let runs = op.runs.load(Ordering::SeqCst);

        e.replace(d, def, vec![v.int(1)]);
        let rep = e.evaluate(threads).unwrap();
        assert_eq!(op.runs.load(Ordering::SeqCst), runs, "nothing to run for: {rep:?}");
        assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(10), "{rep:?}");
        assert_eq!(
            e.output(t),
            first,
            "what it defined and contributed is its again: {rep:?}"
        );
        assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());

        // (and back once more: the record between stands in turn)
        e.replace(d, def, vec![v.int(2)]);
        let rep = e.evaluate(threads).unwrap();
        assert_eq!(op.runs.load(Ordering::SeqCst), runs, "{rep:?}");
        assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(20), "{rep:?}");
        assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());

        // (a value seen by neither runs it)
        e.replace(d, def, vec![v.int(3)]);
        let rep = e.evaluate(threads).unwrap();
        assert_eq!(op.runs.load(Ordering::SeqCst), runs + 1, "{rep:?}");
        assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(30), "{rep:?}");
        let out = e.output(t).expect("tail ran");
        assert_eq!(v.with_list(out, |l| l[0]), Some(v.int(30)), "{rep:?}");
        assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
    }
}

/// A run that placed a node is not taken again from an older record (its
/// placements name the structure it stood in): it runs.
#[test]
fn a_record_that_placed_is_not_taken_again() {
    let (e, op, def, d, r, _t) = setup(true);
    let v = e.values();
    e.evaluate(1).unwrap();
    e.replace(d, def, vec![v.int(2)]);
    e.evaluate(1).unwrap();
    let runs = op.runs.load(Ordering::SeqCst);
    e.replace(d, def, vec![v.int(1)]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(op.runs.load(Ordering::SeqCst), runs + 1, "{rep:?}");
    assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(10), "{rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// `[k]`: reads `x`, outputs k x.
struct Scaled;

impl Operation for Scaled {
    fn name(&self) -> &'static str {
        "scaled"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let k = v.as_int(cx.args()[0]).unwrap_or(0);
        let x = cx.read(v.field("x"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
        Ok(v.int(k * x))
    }
}

/// A record made before an edit of its node is not taken again, though
/// every read of it gives what it gave: a run is a function of its text
/// and its reads (`RunRecord::made_from`). The edited reader's next run
/// on a value its run before the edit read made what that run made, with
/// the old text (phitex-live's edit that streamed no page, and a
/// `\write18` listing edited that printed what the old one did).
#[test]
fn a_record_made_before_an_edit_is_not_taken_again() {
    for threads in [1, 4] {
        let e = Engine::new();
        let v = e.values();
        let def = e.register(Arc::new(Def));
        let scaled = e.register(Arc::new(Scaled));
        let u = e.new_unit();
        let d = e.push(u, def, vec![v.int(1)]);
        let r = e.push(u, scaled, vec![v.int(10)]);
        e.evaluate(threads).unwrap();
        e.replace(d, def, vec![v.int(2)]);
        e.evaluate(threads).unwrap();
        assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(20));
        // (the reader edited, and x moved: its run reads x = 1 with k = 100)
        e.replace(r, scaled, vec![v.int(100)]);
        e.replace(d, def, vec![v.int(1)]);
        e.evaluate(threads).unwrap();
        assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(100), "threads {threads}");
        // (x back to 2: the record before the last read 2, with k = 10)
        e.replace(d, def, vec![v.int(2)]);
        let rep = e.evaluate(threads).unwrap();
        assert_eq!(
            e.output(r).and_then(|x| v.as_int(x)),
            Some(200),
            "threads {threads}: {rep:?}"
        );
        assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
    }
}

/// `[]`: reads `x`, tells the client (`Ctx::told_outside`), outputs x;
/// counts its runs.
struct Telling(AtomicU64);

impl Operation for Telling {
    fn name(&self) -> &'static str {
        "telling"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let x = cx.read(cx.values().field("x"))?.unwrap_or(ValueId::UNIT);
        cx.told_outside();
        Ok(x)
    }
}

/// A run that told the client something (a page streamed) is not taken
/// again from an older record: taken, it would tell nothing. It runs.
#[test]
fn a_record_that_told_the_client_is_not_taken_again() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let op = Arc::new(Telling(AtomicU64::new(0)));
    let telling = e.register(op.clone());
    let u = e.new_unit();
    let d = e.push(u, def, vec![v.int(1)]);
    let r = e.push(u, telling, vec![]);
    e.evaluate(1).unwrap();
    e.replace(d, def, vec![v.int(2)]);
    e.evaluate(1).unwrap();
    let runs = op.0.load(Ordering::SeqCst);
    e.replace(d, def, vec![v.int(1)]);
    let rep = e.evaluate(1).unwrap();
    assert_eq!(op.0.load(Ordering::SeqCst), runs + 1, "{rep:?}");
    assert_eq!(e.output(r), Some(v.int(1)), "{rep:?}");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}
