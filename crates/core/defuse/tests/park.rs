//! A long loop over the line that meets a hole commits what it has and
//! goes on from there when the hole fills (`Ctx::park`, `Ctx::resumed`),
//! and a run knows why it was asked for (`Ctx::woken_by`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{Cause, Ctx, Engine, Field, NodeId, Operation, Stop, ValueId};

/// `[s, x, ms]`: works `ms` milliseconds, contributes x to s.
struct Contrib;

impl Operation for Contrib {
    fn name(&self) -> &'static str {
        "contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ms = cx.values().as_int(cx.args()[2]).unwrap_or(0).unsigned_abs();
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            std::hint::spin_loop();
        }
        cx.contribute(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }

    fn contributes(&self, _: &[ValueId], _: &phitex_defuse::Values, _: Field) -> bool {
        true
    }
}

/// `[s]`: folds the contributions to s in line order, one `total` field
/// defined at the end with their sum. At a hole it parks: its state is
/// `[cursor, sum]`, and the next run goes on from the cursor. Counts the
/// items folded over all runs, the runs that resumed, and keeps the last
/// cause it was woken by.
struct Loop {
    folded: AtomicU64,
    resumed: AtomicU64,
    runs: AtomicU64,
    cause: std::sync::Mutex<Option<Cause>>,
}

impl Operation for Loop {
    fn name(&self) -> &'static str {
        "loop"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::Relaxed);
        *self.cause.lock().unwrap() = cx.woken_by();
        let v = cx.values();
        let s = Field(cx.args()[0]);
        // (a contribution behind the cursor moved: back to the start; the
        // page loop keeps a checkpoint per page and goes back to the one
        // before the mover)
        let state = match (cx.resumed(), cx.woken_by()) {
            (Some(st), Some(Cause::Range { by, .. })) => {
                let parts = v.with_list(st, <[ValueId]>::to_vec).unwrap_or_default();
                let c = v.as_int(parts[0]).unwrap_or(-1);
                if c >= 0 && !cx.before(NodeId::from_raw(c.unsigned_abs()), by) {
                    None
                } else {
                    Some(st)
                }
            }
            (st, _) => st,
        };
        let (mut cur, mut sum): (Option<NodeId>, i64) = match state {
            Some(st) => {
                self.resumed.fetch_add(1, Ordering::Relaxed);
                let parts = v.with_list(st, <[ValueId]>::to_vec).unwrap_or_default();
                let c = v.as_int(parts[0]).unwrap_or(-1);
                (
                    if c < 0 {
                        None
                    } else {
                        Some(NodeId::from_raw(c.unsigned_abs()))
                    },
                    v.as_int(parts[1]).unwrap_or(0),
                )
            }
            None => (None, 0),
        };
        while let Some((n, x)) = cx.next_contribution(s, cur)? {
            self.folded.fetch_add(1, Ordering::Relaxed);
            sum += v.as_int(x).unwrap_or(0);
            cur = Some(n);
        }
        let state = v.list(vec![
            v.int(cur.map_or(-1, |n| i64::try_from(n.raw()).unwrap())),
            v.int(sum),
        ]);
        if cx.stopped_at().is_some() {
            // (a hole: what is folded so far stands, the rest when it fills)
            cx.park(state);
            return Ok(ValueId::UNIT);
        }
        cx.park(state);
        cx.define(v.field("total"), v.int(sum));
        Ok(v.int(sum))
    }

    fn contributes(&self, _: &[ValueId], _: &phitex_defuse::Values, _: Field) -> bool {
        false
    }
}

fn loop_op() -> Arc<Loop> {
    Arc::new(Loop {
        folded: AtomicU64::new(0),
        resumed: AtomicU64::new(0),
        runs: AtomicU64::new(0),
        cause: std::sync::Mutex::new(None),
    })
}

/// Three contributors, the middle one slow, then the loop: the loop meets
/// the hole, parks with two items folded, and the run when the hole fills
/// folds the third alone. Every item is folded once in all.
#[test]
fn a_parked_loop_goes_on_from_its_state_when_the_hole_fills() {
    let e = Engine::new();
    let v = e.values();
    let contrib = e.register(Arc::new(Contrib));
    let lp = loop_op();
    let op = e.register(lp.clone());
    let s = v.field("s");
    let u = e.new_unit();
    e.push(u, contrib, vec![s.0, v.int(1), v.int(0)]);
    e.push(u, contrib, vec![s.0, v.int(2), v.int(400)]);
    e.push(u, contrib, vec![s.0, v.int(3), v.int(0)]);
    let l = e.push(u, op, vec![s.0]);
    let r = e.evaluate(4).unwrap();
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(6), "{r:?}");
    assert_eq!(
        lp.folded.load(Ordering::Relaxed),
        3,
        "each item folded once over all runs: {r:?}"
    );
    assert!(
        lp.runs.load(Ordering::Relaxed) >= 2 && lp.resumed.load(Ordering::Relaxed) >= 1,
        "the loop parked at the hole and resumed: runs {} resumed {} {r:?}",
        lp.runs.load(Ordering::Relaxed),
        lp.resumed.load(Ordering::Relaxed)
    );
    assert!(e.check_invariants().is_empty());
}

/// A contributor's value changes: the loop's run knows the cause (the
/// fold over s changed, by that node) and its result is the new sum.
#[test]
fn a_rerun_knows_the_contribution_that_moved() {
    let e = Engine::new();
    let v = e.values();
    let contrib = e.register(Arc::new(Contrib));
    let lp = loop_op();
    let op = e.register(lp.clone());
    let s = v.field("s");
    let u = e.new_unit();
    e.push(u, contrib, vec![s.0, v.int(1), v.int(0)]);
    let c2 = e.push(u, contrib, vec![s.0, v.int(2), v.int(0)]);
    e.push(u, contrib, vec![s.0, v.int(3), v.int(0)]);
    let l = e.push(u, op, vec![s.0]);
    e.evaluate(2).unwrap();
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(6));
    e.replace(c2, contrib, vec![s.0, v.int(20), v.int(0)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(24), "{r:?}");
    let cause = lp.cause.lock().unwrap().clone();
    assert!(
        matches!(cause, Some(Cause::Range { field, .. }) if field == s),
        "the loop's last run was woken by the fold over s: {cause:?}"
    );
    assert!(e.check_invariants().is_empty());
}

/// An edit of the loop's own text drops its parked state: the next run
/// starts from nothing.
#[test]
fn an_edit_of_the_node_drops_its_parked_state() {
    let e = Engine::new();
    let v = e.values();
    let contrib = e.register(Arc::new(Contrib));
    let lp = loop_op();
    let op = e.register(lp.clone());
    let s = v.field("s");
    let u = e.new_unit();
    e.push(u, contrib, vec![s.0, v.int(1), v.int(0)]);
    let l = e.push(u, op, vec![s.0]);
    e.evaluate(2).unwrap();
    let before = lp.resumed.load(Ordering::Relaxed);
    e.replace(l, op, vec![s.0, v.int(1)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(
        lp.resumed.load(Ordering::Relaxed),
        before,
        "a replaced node resumes from nothing: {r:?}"
    );
    assert!(e.check_invariants().is_empty());
}
