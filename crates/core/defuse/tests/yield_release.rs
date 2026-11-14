//! A run that gives its worker to an earlier node waiting for one
//! (`Ctx::yield_to_prefix`) is queued again once that node is taken to
//! run, not only at its commit: the node it yielded to may stop again at
//! once and wait long (a `\write18`'s child waiting for the settled
//! prefix), and the yielder stood still meanwhile (Grail at -j8: a picture
//! child yielding to such a node, 3 s idle).
//!
//! The order is forced by the test's own operations, with no timing:
//! two workers; `Gate` holds until the test lets it go; `Wait1` and
//! `Wait2` wait for the settled prefix (parked on the gate); `Yielder`,
//! the furthest ahead, checks for starved work at every step. Once it
//! runs, the gate goes: its commit queues both waiters on its worker,
//! which takes `Wait1` (held until the end), leaving `Wait2` queued while
//! both workers are at work. The yielder gives its worker to `Wait2`,
//! which waits again at once, for `Wait1`. The yielder must run again
//! before `Wait1` lets go.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{Ctx, Engine, Operation, Stop, ValueId};

static GATE: AtomicBool = AtomicBool::new(false);
static END: AtomicBool = AtomicBool::new(false);
static PARKED: AtomicU32 = AtomicU32::new(0);
static YIELDER_GAVE: AtomicBool = AtomicBool::new(false);
static YIELDER_AGAIN: AtomicBool = AtomicBool::new(false);

fn hold_until(mut what: impl FnMut() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(20) {
        if what() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    false
}

/// Holds until the test lets it go.
struct Gate;

impl Operation for Gate {
    fn name(&self) -> &'static str {
        "gate"
    }

    fn run(&self, _cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        hold_until(|| GATE.load(Ordering::SeqCst));
        Ok(ValueId::UNIT)
    }
}

/// `[hold]`: waits for the settled prefix; then, with `hold`, holds until
/// the test ends.
struct Settled;

impl Operation for Settled {
    fn name(&self) -> &'static str {
        "settled"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        if let Err(s) = cx.wait_settled() {
            if !GATE.load(Ordering::SeqCst) {
                PARKED.fetch_add(1, Ordering::SeqCst);
            }
            return Err(s);
        }
        if cx.values().as_int(cx.args()[0]) == Some(1) {
            hold_until(|| END.load(Ordering::SeqCst));
        }
        Ok(ValueId::UNIT)
    }
}

/// Once both waiters have parked on the gate, lets the gate go and
/// steps until the test ends, giving its worker away where earlier work
/// waits for one.
struct Yielder;

impl Operation for Yielder {
    fn name(&self) -> &'static str {
        "yielder"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        if YIELDER_GAVE.load(Ordering::SeqCst) {
            YIELDER_AGAIN.store(true, Ordering::SeqCst);
            return Ok(ValueId::UNIT);
        }
        // (the two waiters parked on the gate first: run again until then)
        if PARKED.load(Ordering::SeqCst) < 2 {
            return Err(cx.run_again());
        }
        GATE.store(true, Ordering::SeqCst);
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            if let Err(s) = cx.yield_to_prefix() {
                YIELDER_GAVE.store(true, Ordering::SeqCst);
                return Err(s);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_run_that_gave_its_worker_runs_again_once_that_node_is_taken() {
    let e = Engine::new();
    let v = e.values();
    let gate = e.register(Arc::new(Gate));
    let settled = e.register(Arc::new(Settled));
    let yielder = e.register(Arc::new(Yielder));
    let u = e.new_unit();
    e.push(u, gate, vec![]);
    e.push(u, settled, vec![v.int(1)]);
    e.push(u, settled, vec![v.int(0)]);
    let y = e.push(u, yielder, vec![]);
    let watch = std::thread::spawn(|| {
        // (the yielder's second run, while `Wait1` still holds)
        let again = hold_until(|| YIELDER_AGAIN.load(Ordering::SeqCst));
        END.store(true, Ordering::SeqCst);
        again
    });
    let r = e.evaluate(2).unwrap();
    let again = watch.join().unwrap();
    assert!(
        YIELDER_GAVE.load(Ordering::SeqCst),
        "the yielder gave its worker: {r:?}"
    );
    assert!(
        again,
        "the yielder ran again before the node it gave its worker to committed: {r:?}"
    );
    assert!(e.output(y).is_some());
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}
