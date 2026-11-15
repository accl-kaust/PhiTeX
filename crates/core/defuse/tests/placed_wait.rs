//! A placer that waits for a placement's next commit (`Ctx::placed_output_wait`)
//! on a guess, because that placement is to run again: if that commit comes
//! while the placer's run is still going, before the placer's commit has
//! published the record that says it waits, the placer is still asked
//! again, and does not keep its guess.
//!
//! The interleaving is forced by the test's own operations, with no timing
//! involved: the child's run holds until the placer's run has looked at it,
//! and the placer's run holds until the child has committed and settled.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{Ctx, Engine, Field, OpId, Operation, Stop, ValueId};

/// The phase the test is in: 1 the clean build, 2 the edit whose window is
/// forced.
static PHASE: AtomicU32 = AtomicU32::new(0);
/// The placer's run in phase 2 has looked at its child (found it to run).
static LOOKED: AtomicBool = AtomicBool::new(false);
/// The child committed while the placer's run held (found not pending).
static IN_WINDOW: AtomicBool = AtomicBool::new(false);

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

/// `[f, v]`: defines f = v; outputs v.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, v) = (Field(cx.args()[0]), cx.args()[1]);
        cx.define(f, v);
        Ok(v)
    }
}

/// `[f]`: outputs the value of f reaching it. In phase 2 its run holds
/// until the placer's run has looked at it.
struct Kid;

impl Operation for Kid {
    fn name(&self) -> &'static str {
        "kid"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT);
        if PHASE.load(Ordering::SeqCst) == 2 {
            hold_until(|| LOOKED.load(Ordering::SeqCst));
        }
        Ok(x)
    }
}

/// `[f]`: reads f, places `Kid [f]` and outputs its output. While the
/// child has not run or is to run again, it goes on without that output
/// (UNIT) and waits for the child's next commit, as a picture whose view
/// moved does (`pic.rs`). In phase 2 its first such run holds, after the
/// look, until the child has committed and settled.
struct Waiter(std::sync::OnceLock<OpId>);

impl Operation for Waiter {
    fn name(&self) -> &'static str {
        "waiter"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        // (a long run, as a paragraph of pictures is: in phase 2 a thief
        // takes the child its worker's queue holds, `Ready::steal`)
        if PHASE.load(Ordering::SeqCst) == 1 {
            std::thread::sleep(Duration::from_millis(5));
        }
        let f = cx.args()[0];
        cx.read(Field(f))?;
        let kid = *self.0.get().expect("the kid");
        let i = cx.child_once(kid, vec![f]);
        if cx.placed_output_peek(i).is_none() || cx.placed_pending(i) {
            cx.placed_output_guessed(i, ValueId::UNIT);
            cx.placed_output_wait(i);
            if PHASE.load(Ordering::SeqCst) == 2 && !LOOKED.swap(true, Ordering::SeqCst) {
                // (the child commits here, its commit finding this node
                // running and its last record the one before this run)
                if hold_until(|| !cx.placed_pending(i)) {
                    IN_WINDOW.store(true, Ordering::SeqCst);
                }
            }
            return Ok(ValueId::UNIT);
        }
        Ok(cx.placed_output_or(i, ValueId::UNIT))
    }
}

/// The child commits between its placer's look at it (to run again: the
/// placer goes on on a guess, waiting for that commit) and the placer's
/// commit. That commit read the placer's record before this run's. The
/// placer is still asked again and outputs the child's new output, not
/// the guess. Before the commit sequence (`Node::commit_seq`), the
/// placer's commit skipped the check of a guess it waits for and stood
/// on UNIT: a picture of pgf-umlcd-manual empty under its \resizebox,
/// about 1 build in 75 at -j4 on 4 CPUs.
#[test]
fn a_placement_committing_during_its_waiting_placer_s_run_asks_it_again() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let kid = e.register(Arc::new(Kid));
    let w = Arc::new(Waiter(std::sync::OnceLock::new()));
    w.0.set(kid).unwrap();
    let wo = e.register(w);
    let f = v.field("f");
    let u = e.new_unit();
    let d = e.push(u, def, vec![f.0, v.int(1)]);
    let p = e.push(u, wo, vec![f.0]);
    PHASE.store(1, Ordering::SeqCst);
    let r = e.evaluate(4).unwrap();
    assert_eq!(e.output(p).and_then(|x| v.as_int(x)), Some(1), "the clean build: {r:?}");
    // the edit: the child and its placer both read f, both run again
    PHASE.store(2, Ordering::SeqCst);
    e.replace(d, def, vec![f.0, v.int(2)]);
    let r = e.evaluate(4).unwrap();
    PHASE.store(0, Ordering::SeqCst);
    assert!(
        LOOKED.load(Ordering::SeqCst) && IN_WINDOW.load(Ordering::SeqCst),
        "the child committed inside the placer's run: {r:?}"
    );
    assert_eq!(
        e.output(p).and_then(|x| v.as_int(x)),
        Some(2),
        "the placer took the child's new output, not its guess: {r:?}"
    );
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}
