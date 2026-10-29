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
    /// The walk's runs offered a state, woken by a range that moved,
    /// with the site the cause named (raw ids).
    offered: std::sync::Mutex<Vec<u64>>,
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
            (
                if c < 0 {
                    None
                } else {
                    Some(NodeId::from_raw(c.unsigned_abs()))
                },
                v.as_int(parts[1]).unwrap_or(0),
            )
        };
        if let (Some(_), Some(Cause::Range { by, .. })) = (cx.resumed(), cx.woken_by()) {
            self.0.offered.lock().unwrap().push(by.raw());
        }
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
        cx.park(v.list(vec![
            v.int(cur.map_or(-1, |n| i64::try_from(n.raw()).unwrap())),
            v.int(sum),
        ]));
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
    Doc {
        e,
        g,
        s,
        walk,
        contrib,
        contrib_place,
        quiet,
        t,
        d0,
        c1,
        c2,
        l,
    }
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

/// `[s]`: reads "t"; contributes 1 to s, and where t is not 0 places a
/// contributor of 5 after itself, waiting first (where `contrib_waits`)
/// for the walk to have read, and raising `contrib_done` at its end.
struct PlaceOnT(OpId, Arc<Gates>);

impl Operation for PlaceOnT {
    fn name(&self) -> &'static str {
        "place_on_t"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let t = cx.read(v.field("t"))?;
        let s = cx.args()[0];
        if t.and_then(|x| v.as_int(x)).unwrap_or(0) != 0 {
            if self.1.contrib_waits.load(Ordering::Acquire) {
                spin_until(&self.1.loop_read, 30);
            }
            cx.child(self.0, vec![s, v.int(5), v.int(0)]);
        }
        cx.contribute(Field(s), v.int(1));
        self.1.contrib_done.store(true, Ordering::Release);
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_kept_range_read_refuted_at_the_commit_keeps_the_state_and_names_its_start() {
    // The walk resumes from its state (a contribution added ahead of its
    // cursor) and reads from its cursor on; the reads its state rests on
    // are kept, the next contribution after the placer among them.
    // While it runs, the placer runs again and places a contributor
    // after itself: the walk's commit finds that kept read moved. The
    // next run is offered the state and told the change lies after the
    // placer (the read's start), and goes back before it. (A loop with a
    // checkpoint per page goes back to the one before the site; an
    // emptied state made it start over.)
    let e = Engine::new();
    let g = Arc::new(Gates::default());
    let v = e.values();
    let s = v.field("s");
    let walk = e.register(Arc::new(Walk(g.clone())));
    let contrib = e.register(Arc::new(Contrib(g.clone())));
    let place = e.register(Arc::new(PlaceOnT(contrib, g.clone())));
    let def_t = e.register(Arc::new(DefT));
    let u = e.new_unit();
    let t = e.push(u, def_t, vec![v.int(0)]);
    let p = e.push(u, place, vec![s.0]);
    e.push(u, contrib, vec![s.0, v.int(3), v.int(0)]);
    let l = e.push(u, walk, vec![s.0, v.int(1)]);
    e.evaluate(2).unwrap();
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(4));
    g.loop_waits.store(true, Ordering::Release);
    g.contrib_waits.store(true, Ordering::Release);
    g.contrib_done.store(false, Ordering::Release);
    e.replace(t, def_t, vec![v.int(1)]);
    e.insert_before(l, contrib, vec![s.0, v.int(100), v.int(0)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(109), "{r:?}");
    let offered = g.offered.lock().unwrap().clone();
    assert!(
        offered.contains(&p.raw()),
        "a run offered the state with the placer as the site: {offered:?} (placer {}) {r:?}",
        p.raw()
    );
    assert!(e.check_invariants().is_empty());
}

/// `[s, t0]`: contributes 1 to s, and where "t" equals t0 places a
/// contributor of 5 after itself; where it does not, waits first (where
/// `contrib_waits`) for the walk to have read. Raises `contrib_done`.
struct PlaceWhileT(OpId, Arc<Gates>);

impl Operation for PlaceWhileT {
    fn name(&self) -> &'static str {
        "place_while_t"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let t = cx.read(v.field("t"))?;
        let s = cx.args()[0];
        if t == Some(cx.args()[1]) {
            cx.child(self.0, vec![s, v.int(5), v.int(0)]);
        } else if self.1.contrib_waits.load(Ordering::Acquire) {
            spin_until(&self.1.loop_read, 30);
        }
        cx.contribute(Field(s), v.int(1));
        self.1.contrib_done.store(true, Ordering::Release);
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_kept_read_from_a_node_gone_is_not_kept() {
    // The placer's contributor of 5 stands after it, and the walk read
    // the next contribution after that contributor. The placer runs again
    // and places nothing: the contributor is gone. The walk, woken (by
    // "t", then by the read from the placer), runs again; a read from the
    // node gone is not kept with its record: no run reads from it again,
    // and kept it moved at the next commit and ran the walk again (twice
    // more here) (a loop that goes back to a
    // checkpoint and stops early where a page fires as it was ran again
    // at each, without end).
    let e = Engine::new();
    let g = Arc::new(Gates::default());
    let v = e.values();
    let s = v.field("s");
    let walk = e.register(Arc::new(Walk(g.clone())));
    let contrib = e.register(Arc::new(Contrib(g.clone())));
    let place = e.register(Arc::new(PlaceWhileT(contrib, g.clone())));
    let def_t = e.register(Arc::new(DefT));
    let u = e.new_unit();
    let t = e.push(u, def_t, vec![v.int(0)]);
    let p = e.push(u, place, vec![s.0, v.int(0)]);
    e.push(u, contrib, vec![s.0, v.int(3), v.int(0)]);
    let l = e.push(u, walk, vec![s.0, v.int(1)]);
    e.evaluate(2).unwrap();
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(9));
    let c = e.children(p);
    assert_eq!(c.len(), 1, "the placer's contributor");
    let from_c = format!("after: Some({:?})", c[0]);
    assert!(
        e.debug_reads(l).contains(&from_c),
        "the walk read from it: {}",
        e.debug_reads(l)
    );
    e.replace(t, def_t, vec![v.int(1)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(4), "{r:?}");
    let walks = r.runs_by_op.iter().find(|x| x.0 == "walk").map_or(0, |x| x.1);
    assert!(
        walks <= 2,
        "the walk runs for the edit and for the read from the placer, not again for the read from the node gone: {r:?}"
    );
    assert!(
        !e.debug_reads(l).contains(&from_c),
        "no read from the node gone: {}",
        e.debug_reads(l)
    );
    assert!(e.check_invariants().is_empty());
}

/// `[s, a]`: folds the contributions to s, parked as `[cursor, sum]`;
/// woken by a range it resumes from its state, else starts over, then
/// reading "t" where node `a` stands (`At::Before`) while `a` stands.
/// Keeps every cause it was woken by. With gates: raises `loop_read`
/// after reading, and waits for `contrib_done` where `loop_waits`.
struct AnchorWalk(std::sync::Mutex<Vec<Option<Cause>>>, Option<Arc<Gates>>);

impl Operation for AnchorWalk {
    fn name(&self) -> &'static str {
        "anchor_walk"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let s = Field(cx.args()[0]);
        let a = NodeId::from_raw(v.as_int(cx.args()[1]).unwrap_or(0).unsigned_abs());
        self.0.lock().unwrap().push(cx.woken_by());
        let state = match (cx.resumed(), cx.woken_by()) {
            (Some(st), Some(Cause::Range { .. })) => Some(st),
            _ => None,
        };
        let (mut cur, mut sum) = state.map_or((None, 0), |st| {
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
        });
        if state.is_none() && cx.alive(a) {
            let _ = cx.read_at(v.field("t"), phitex_defuse::At::Before(a))?;
        }
        if let Some(g) = &self.1 {
            g.loop_read.store(true, Ordering::Release);
            if g.loop_waits.load(Ordering::Acquire) {
                spin_until(&g.contrib_done, 30);
            }
        }
        while let Some((n, x)) = cx.next_contribution(s, cur)? {
            sum += v.as_int(x).unwrap_or(0);
            cur = Some(n);
        }
        cx.park(v.list(vec![
            v.int(cur.map_or(-1, |n| i64::try_from(n.raw()).unwrap())),
            v.int(sum),
        ]));
        Ok(v.int(sum))
    }

    fn contributes(&self, _: &[ValueId], _: &phitex_defuse::Values, _: Field) -> bool {
        false
    }
}

#[test]
fn a_kept_read_at_a_node_gone_wakes_a_parking_run_once_with_its_site() {
    // The walk read "t" where a quiet node stands. The node is removed and
    // a contribution added ahead of the walk's cursor: the walk resumes,
    // keeping its read at the node gone. That read is not kept: it asks
    // the walk again once, the change where the node stood (a loop with a
    // checkpoint per page goes back before it); a moved structure, with no
    // site, sent the page loop back to its start at every commit.
    let e = Engine::new();
    let g = Arc::new(Gates::default());
    let v = e.values();
    let s = v.field("s");
    let walk = Arc::new(AnchorWalk(std::sync::Mutex::new(Vec::new()), None));
    let walk_op = e.register(walk.clone());
    let contrib = e.register(Arc::new(Contrib(g.clone())));
    let quiet = e.register(Arc::new(Quiet));
    let def_t = e.register(Arc::new(DefT));
    let u = e.new_unit();
    e.push(u, def_t, vec![v.int(0)]);
    e.push(u, contrib, vec![s.0, v.int(1), v.int(0)]);
    let a = e.push(u, quiet, vec![]);
    e.push(u, contrib, vec![s.0, v.int(2), v.int(0)]);
    let l = e.push(u, walk_op, vec![s.0, v.int(i64::try_from(a.raw()).unwrap())]);
    e.evaluate(2).unwrap();
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(3));
    walk.0.lock().unwrap().clear();
    e.remove(a);
    e.insert_before(l, contrib, vec![s.0, v.int(100), v.int(0)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(103), "{r:?}");
    let causes = walk.0.lock().unwrap().clone();
    assert!(
        causes
            .iter()
            .any(|c| matches!(c, Some(Cause::Changed { site: Some(n), .. }) if *n == a)),
        "woken where the node stood: {causes:?}"
    );
    assert!(causes.len() <= 3, "{causes:?}");
    assert!(e.check_invariants().is_empty());
}

/// `[x]`: defines "t" = x + "k"; where `contrib_waits`, waits first for
/// the walk to have read; raises `contrib_done`.
struct GatedDefT(Arc<Gates>);

impl Operation for GatedDefT {
    fn name(&self) -> &'static str {
        "gated_def_t"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        if self.0.contrib_waits.load(Ordering::Acquire) {
            spin_until(&self.0.loop_read, 30);
        }
        let v = cx.values();
        let k = cx.read(v.field("k"))?.and_then(|k| v.as_int(k)).unwrap_or(0);
        let x = v.int(v.as_int(cx.args()[0]).unwrap_or(0) + k);
        cx.define(v.field("t"), x);
        self.0.contrib_done.store(true, Ordering::Release);
        Ok(x)
    }
}

/// `[x]`: defines "k" = x.
struct DefK;

impl Operation for DefK {
    fn name(&self) -> &'static str {
        "def_k"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.args()[0];
        cx.define(cx.values().field("k"), x);
        Ok(x)
    }
}

#[test]
fn a_read_at_a_node_refuted_at_the_commit_names_the_node_to_a_parking_run() {
    // The walk reads "t" where a quiet node stands; while it runs, "t"
    // is defined anew before that node, and the walk's commit finds its
    // read moved. A run that parks a state is told where: at the node it
    // read at, or the definer (the earlier), never no site (which sent
    // the page loop back to its start where a checkpoint before the
    // piece it read at would do).
    let e = Engine::new();
    let g = Arc::new(Gates::default());
    let v = e.values();
    let s = v.field("s");
    let walk = Arc::new(AnchorWalk(std::sync::Mutex::new(Vec::new()), Some(g.clone())));
    let walk_op = e.register(walk.clone());
    let contrib = e.register(Arc::new(Contrib(g.clone())));
    let quiet = e.register(Arc::new(Quiet));
    let def_t = e.register(Arc::new(GatedDefT(g.clone())));
    let def_k = e.register(Arc::new(DefK));
    let u = e.new_unit();
    let k = e.push(u, def_k, vec![v.int(0)]);
    e.push(u, def_t, vec![v.int(0)]);
    e.push(u, contrib, vec![s.0, v.int(1), v.int(0)]);
    let a = e.push(u, quiet, vec![]);
    e.push(u, contrib, vec![s.0, v.int(2), v.int(0)]);
    let l = e.push(u, walk_op, vec![s.0, v.int(i64::try_from(a.raw()).unwrap())]);
    e.evaluate(2).unwrap();
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(3));
    walk.0.lock().unwrap().clear();
    g.loop_waits.store(true, Ordering::Release);
    g.contrib_waits.store(true, Ordering::Release);
    g.contrib_done.store(false, Ordering::Release);
    g.loop_read.store(false, Ordering::Release);
    e.replace(k, def_k, vec![v.int(1)]);
    e.replace(l, walk_op, vec![s.0, v.int(i64::try_from(a.raw()).unwrap())]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(3), "{r:?}");
    let causes = walk.0.lock().unwrap().clone();
    assert!(
        !causes
            .iter()
            .any(|c| matches!(c, Some(Cause::Changed { site: None, .. }))),
        "a site for every changed read: {causes:?}"
    );
    assert!(e.check_invariants().is_empty());
}
