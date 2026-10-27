//! The second review of a parked loop (`Ctx::park`, `Ctx::resumed`,
//! `Ctx::woken_by`): a loop that resumes from its state must be told every
//! change behind its cursor, and an edit of its text.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{Cause, Ctx, Engine, Field, NodeId, OpId, Operation, Stop, ValueId};

fn spin_until(flag: &AtomicBool, then_ms: u64) {
    let t = Instant::now();
    while !flag.load(Ordering::Acquire) && t.elapsed() < Duration::from_secs(5) {
        std::hint::spin_loop();
    }
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(then_ms) {
        std::hint::spin_loop();
    }
}

/// The flags the tests' operations wait on and raise.
#[derive(Default)]
struct Gates {
    /// The loop has read what it reads (raised by it).
    loop_read: AtomicBool,
    /// Raised by a gated contributor's run when it ends.
    contrib_done: AtomicBool,
    /// The loop waits for `contrib_done` after reading, where up.
    loop_waits: AtomicBool,
    /// A gated contributor waits for `loop_read`, where up.
    contrib_waits: AtomicBool,
    /// Raised by a placer's run when it ends; a gated contributor waits
    /// for it, where `contrib_waits_placer` is up.
    placer_done: AtomicBool,
    contrib_waits_placer: AtomicBool,
}

/// `[s, x, gated]`: contributes x to s; gated (1), it waits as `Gates` say
/// and raises `contrib_done` at its end.
struct Contrib(Arc<Gates>);

impl Operation for Contrib {
    fn name(&self) -> &'static str {
        "contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        if v.as_int(cx.args()[2]) == Some(1) {
            if self.0.contrib_waits.load(Ordering::Acquire) {
                spin_until(&self.0.loop_read, 30);
            }
            if self.0.contrib_waits_placer.load(Ordering::Acquire) {
                spin_until(&self.0.placer_done, 30);
            }
        }
        cx.contribute(Field(cx.args()[0]), cx.args()[1]);
        if v.as_int(cx.args()[2]) == Some(1) {
            self.0.contrib_done.store(true, Ordering::Release);
        }
        Ok(cx.args()[1])
    }
}

/// `[ms]`: works `ms` milliseconds.
struct Slow;

impl Operation for Slow {
    fn name(&self) -> &'static str {
        "slow"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ms = cx.values().as_int(cx.args()[0]).unwrap_or(0).unsigned_abs();
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            std::hint::spin_loop();
        }
        Ok(ValueId::UNIT)
    }
}

/// `[s, x, ms]`: contributes x to s and places a `Slow` of `ms`; raises
/// `placer_done` at its end.
struct ContribPlace(OpId, Arc<Gates>);

impl Operation for ContribPlace {
    fn name(&self) -> &'static str {
        "contrib_place"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.contribute(Field(cx.args()[0]), cx.args()[1]);
        cx.child(self.0, vec![cx.args()[2]]);
        self.1.placer_done.store(true, Ordering::Release);
        Ok(cx.args()[1])
    }
}

/// `[x]`: defines "t" = x.
struct DefT;

impl Operation for DefT {
    fn name(&self) -> &'static str {
        "def_t"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.args()[0];
        cx.define(cx.values().field("t"), x);
        Ok(x)
    }
}

/// `[]`: nothing.
struct Quiet;

impl Operation for Quiet {
    fn name(&self) -> &'static str {
        "quiet"
    }

    fn run(&self, _cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

/// `[s, k]`: reads "t", then folds k times each contribution to s in
/// line order, parked as `[cursor, sum]` and resumed as park.rs's loop
/// does: from its state unless the cause names a contribution at or
/// behind its cursor (then from the start). Its output the sum.
struct Walk(Arc<Gates>);

impl Operation for Walk {
    fn name(&self) -> &'static str {
        "walk"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        cx.read(v.field("t"))?;
        let s = Field(cx.args()[0]);
        let k = v.as_int(cx.args()[1]).unwrap_or(1);
        let decode = |st: ValueId| {
            let parts = v.with_list(st, <[ValueId]>::to_vec).unwrap_or_default();
            let c = v.as_int(parts[0]).unwrap_or(-1);
            (if c < 0 { None } else { Some(NodeId::from_raw(c.unsigned_abs())) }, v.as_int(parts[1]).unwrap_or(0))
        };
        let state = match (cx.resumed(), cx.woken_by()) {
            (Some(st), Some(Cause::Range { by, .. })) => match decode(st).0 {
                Some(c) if cx.before(c, by) => Some(st),
                _ => None,
            },
            (st, _) => st,
        };
        let (mut cur, mut sum) = state.map_or((None, 0), decode);
        while let Some((n, x)) = cx.next_contribution(s, cur)? {
            sum += k * v.as_int(x).unwrap_or(0);
            cur = Some(n);
        }
        self.0.loop_read.store(true, Ordering::Release);
        if self.0.loop_waits.load(Ordering::Acquire) {
            spin_until(&self.0.contrib_done, 30);
        }
        cx.park(v.list(vec![v.int(cur.map_or(-1, |n| i64::try_from(n.raw()).unwrap())), v.int(sum)]));
        Ok(v.int(sum))
    }

    fn contributes(&self, _: &[ValueId], _: &phitex_defuse::Values, _: Field) -> bool {
        false
    }
}

struct Doc {
    e: Engine,
    g: Arc<Gates>,
    s: Field,
    walk: OpId,
    contrib: OpId,
    contrib_place: OpId,
    quiet: OpId,
    t: NodeId,
    d0: NodeId,
    c1: NodeId,
    c2: NodeId,
    l: NodeId,
}

/// `t`, a quiet node, contributions 1 and 2 (the first gated), the walk;
/// evaluated once, every gate down.
fn doc() -> Doc {
    let e = Engine::new();
    let g = Arc::new(Gates::default());
    let v = e.values();
    let s = v.field("s");
    let walk = e.register(Arc::new(Walk(g.clone())));
    let contrib = e.register(Arc::new(Contrib(g.clone())));
    let slow = e.register(Arc::new(Slow));
    let contrib_place = e.register(Arc::new(ContribPlace(slow, g.clone())));
    let quiet = e.register(Arc::new(Quiet));
    let def_t = e.register(Arc::new(DefT));
    let u = e.new_unit();
    let t = e.push(u, def_t, vec![v.int(0)]);
    let d0 = e.push(u, quiet, vec![]);
    let c1 = e.push(u, contrib, vec![s.0, v.int(1), v.int(1)]);
    let c2 = e.push(u, contrib, vec![s.0, v.int(2), v.int(0)]);
    let l = e.push(u, walk, vec![s.0, v.int(1)]);
    e.evaluate(2).unwrap();
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(3));
    Doc { e, g, s, walk, contrib, contrib_place, quiet, t, d0, c1, c2, l }
}

fn out(d: &Doc) -> Option<i64> {
    d.e.output(d.l).and_then(|x| d.e.values().as_int(x))
}

#[test]
fn an_edit_of_a_node_already_dirty_drops_its_parked_state() {
    // The walk is made dirty by a change it read (the definer of `t`
    // removed), then its text is edited before it runs: the edit's cause
    // was dropped for the one already there, so the run resumed from the
    // state its old text parked (the sum of the old multiplier).
    let d = doc();
    let v = d.e.values();
    d.e.remove(d.t);
    d.e.replace(d.l, d.walk, vec![d.s.0, v.int(2)]);
    let r = d.e.evaluate(2).unwrap();
    assert_eq!(out(&d), Some(6), "the edited walk starts over: {r:?}");
    assert!(d.e.check_invariants().is_empty());
}

#[test]
fn a_run_refuted_at_its_commit_is_told_where_the_change_may_lie() {
    // A contribution behind the walk's cursor changes while the walk runs,
    // after it read it: the walk's commit finds its read moved and runs it
    // again, giving as the cause a fold over s changed by the walk itself,
    // a site after its cursor: the run resumed from the stale sum, and as
    // it read only from its cursor on, it stood.
    let d = doc();
    let v = d.e.values();
    d.g.loop_waits.store(true, Ordering::Release);
    d.g.contrib_waits.store(true, Ordering::Release);
    d.e.replace(d.d0, d.quiet, vec![]);
    d.e.replace(d.c1, d.contrib, vec![d.s.0, v.int(10), v.int(1)]);
    d.e.replace(d.l, d.walk, vec![d.s.0, v.int(1)]);
    let r = d.e.evaluate(2).unwrap();
    assert_eq!(out(&d), Some(12), "{r:?}");
    assert!(d.e.check_invariants().is_empty());
}

#[test]
fn a_second_cause_before_the_run_is_not_lost() {
    // A contribution after the walk's cursor is added (the walk is woken
    // by it, a site ahead: it may go on from its state), and, before the
    // walk runs, one behind its cursor changes: that cause was dropped, the
    // node being dirty already, and the walk resumed past the change.
    let d = doc();
    let v = d.e.values();
    d.g.contrib_waits_placer.store(true, Ordering::Release);
    d.e.replace(d.d0, d.quiet, vec![]);
    d.e.replace(d.c1, d.contrib, vec![d.s.0, v.int(10), v.int(1)]);
    // (its placement runs 300 ms on the worker that queued the walk)
    d.e.insert_before(d.l, d.contrib_place, vec![d.s.0, v.int(100), v.int(300)]);
    let r = d.e.evaluate(2).unwrap();
    assert_eq!(out(&d), Some(112), "{r:?}");
    assert!(d.e.check_invariants().is_empty());
}

#[test]
fn a_resumed_run_still_reads_what_its_state_was_made_of() {
    // The walk resumes from its state after a contribution ahead of its
    // cursor is added, and commits; then one behind its cursor changes.
    // The resumed run's reads were only those from its cursor on: the
    // reads its state rested on went with the record it replaced, so the
    // change behind asked nobody and the walk kept the stale sum.
    let d = doc();
    let v = d.e.values();
    d.g.contrib_waits.store(true, Ordering::Release);
    d.e.replace(d.d0, d.quiet, vec![]);
    d.e.replace(d.c1, d.contrib, vec![d.s.0, v.int(10), v.int(1)]);
    d.e.insert_before(d.l, d.contrib, vec![d.s.0, v.int(100), v.int(0)]);
    let r = d.e.evaluate(2).unwrap();
    assert_eq!(out(&d), Some(112), "{r:?}");
    assert!(d.e.check_invariants().is_empty());
    let _ = (d.c2, d.contrib_place);
}
