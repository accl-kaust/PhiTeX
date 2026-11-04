//! A loop that parks at every contributor of a long stretch keeps all the
//! reads since it began: from some length on they stay filed
//! (`ReadRec::filed`), and a commit neither files nor checks them. A change
//! behind the cursor still reaches the loop, when it is parked and while
//! it runs.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{Cause, Ctx, Engine, Field, NodeId, Operation, Stop, ValueId};

#[derive(Default)]
struct Gates {
    /// The loop waits in its run for `moved` when set.
    hold_loop: AtomicBool,
    entered: AtomicBool,
    moved: AtomicBool,
    went_back: AtomicU64,
}

fn spin_until(f: &AtomicBool, secs: u64) -> bool {
    let t = Instant::now();
    while !f.load(Ordering::Acquire) {
        if t.elapsed() > Duration::from_secs(secs) {
            return false;
        }
        std::hint::spin_loop();
    }
    true
}

/// `[s, x, waits]`: contributes x to s; where waits is 1 and the gate is on,
/// only once the loop is in its run, and tells it when done.
struct Contrib(Arc<Gates>);

impl Operation for Contrib {
    fn name(&self) -> &'static str {
        "contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let waits = cx.values().as_int(cx.args()[2]).unwrap_or(0) == 1;
        if waits && self.0.hold_loop.load(Ordering::Acquire) {
            spin_until(&self.0.entered, 20);
        }
        cx.contribute(Field(cx.args()[0]), cx.args()[1]);
        if waits {
            self.0.moved.store(true, Ordering::Release);
        }
        Ok(cx.args()[1])
    }

    fn contributes(&self, _: &[ValueId], _: &phitex_defuse::Values, _: Field) -> bool {
        true
    }
}

/// `[s]`: sums the contributions to s in line order, parking `[cursor, sum]`
/// at its end: its next run goes on from the cursor, or back to the start
/// when told of a change at or behind it.
struct Loop(Arc<Gates>);

impl Operation for Loop {
    fn name(&self) -> &'static str {
        "loop"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let s = Field(cx.args()[0]);
        let state = match (cx.resumed(), cx.woken_by()) {
            (Some(st), Some(Cause::Range { by, .. })) => {
                let parts = v.with_list(st, <[ValueId]>::to_vec).unwrap_or_default();
                let c = v.as_int(parts[0]).unwrap_or(-1);
                if c >= 0 && !cx.before(NodeId::from_raw(c.unsigned_abs()), by) {
                    self.0.went_back.fetch_add(1, Ordering::Relaxed);
                    None
                } else {
                    Some(st)
                }
            }
            (st, _) => st,
        };
        let (mut cur, mut sum) = match state {
            Some(st) => {
                let parts = v.with_list(st, <[ValueId]>::to_vec).unwrap_or_default();
                let c = v.as_int(parts[0]).unwrap_or(-1);
                (
                    (c >= 0).then(|| NodeId::from_raw(c.unsigned_abs())),
                    v.as_int(parts[1]).unwrap_or(0),
                )
            }
            None => (None, 0),
        };
        while let Some((n, x)) = cx.next_contribution(s, cur)? {
            sum += v.as_int(x).unwrap_or(0);
            cur = Some(n);
        }
        if self.0.hold_loop.load(Ordering::Acquire) {
            self.0.entered.store(true, Ordering::Release);
            spin_until(&self.0.moved, 20);
            // (the mover's commit, after its run, walks to this running loop)
            std::thread::sleep(Duration::from_millis(200));
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

/// `[s]`: contributes "t" to s; where the gate is on, only once the loop
/// is in its run, and tells it when done.
struct ContribT(Arc<Gates>);

impl Operation for ContribT {
    fn name(&self) -> &'static str {
        "contrib_t"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let t = cx.read(v.field("t"))?.unwrap_or_else(|| v.int(0));
        if self.0.hold_loop.load(Ordering::Acquire) {
            spin_until(&self.0.entered, 20);
        }
        cx.contribute(Field(cx.args()[0]), t);
        self.0.moved.store(true, Ordering::Release);
        Ok(t)
    }

    fn contributes(&self, _: &[ValueId], _: &phitex_defuse::Values, _: Field) -> bool {
        true
    }
}

struct Doc {
    e: Engine,
    g: Arc<Gates>,
    contrib: phitex_defuse::OpId,
    s: Field,
    first: Vec<NodeId>,
    t: NodeId,
    def_t: phitex_defuse::OpId,
    l: NodeId,
    sum: i64,
}

/// "t" = 0, a contributor of "t", then a loop, and 400 contributors added
/// before the loop one evaluation at a time: each its own commit going on
/// from the last, far past the first filing.
fn doc() -> Doc {
    let e = Engine::new();
    let g = Arc::new(Gates::default());
    let v = e.values();
    let contrib = e.register(Arc::new(Contrib(g.clone())));
    let lp = e.register(Arc::new(Loop(g.clone())));
    let s = v.field("s");
    let def_t = e.register(Arc::new(DefT));
    let ct = e.register(Arc::new(ContribT(g.clone())));
    let u = e.new_unit();
    let t = e.push(u, def_t, vec![v.int(0)]);
    e.push(u, ct, vec![s.0]);
    let c0 = e.push(u, contrib, vec![s.0, v.int(1), v.int(0)]);
    let l = e.push(u, lp, vec![s.0]);
    e.evaluate(1).unwrap();
    let mut first = vec![c0];
    let mut sum = 1;
    for i in 2..=400 {
        first.push(e.insert_before(l, contrib, vec![s.0, v.int(i), v.int(0)]));
        sum += i;
        e.evaluate(1).unwrap();
    }
    assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(sum));
    Doc {
        e,
        g,
        contrib,
        s,
        first,
        t,
        def_t,
        l,
        sum,
    }
}

/// The loop is parked; a contribution far behind its cursor, in the filed
/// reads, changes.
#[test]
fn a_filed_read_that_moves_asks_the_parked_loop() {
    let d = doc();
    let v = d.e.values();
    d.e.replace(d.first[5], d.contrib, vec![d.s.0, v.int(1006), v.int(0)]);
    let r = d.e.evaluate(2).unwrap();
    assert_eq!(d.e.output(d.l).and_then(|x| v.as_int(x)), Some(d.sum + 1000), "{r:?}");
    assert!(d.g.went_back.load(Ordering::Relaxed) >= 1);
    assert!(d.e.check_invariants().is_empty());
}

/// The loop runs, going on from its cursor for a contributor added at the
/// end, when the contribution far behind it of a reader of "t", which
/// changed, commits: the walk finds the loop running and leaves it to its
/// commit, which does not check a filed read unless told.
#[test]
fn a_filed_read_that_moves_during_the_run_refutes_its_commit() {
    let d = doc();
    let v = d.e.values();
    d.g.hold_loop.store(true, Ordering::Release);
    d.e.replace(d.t, d.def_t, vec![v.int(1000)]);
    d.e.insert_before(d.l, d.contrib, vec![d.s.0, v.int(500), v.int(0)]);
    let r = d.e.evaluate(2).unwrap();
    assert_eq!(
        d.e.output(d.l).and_then(|x| v.as_int(x)),
        Some(d.sum + 1000 + 500),
        "{r:?}"
    );
    assert!(
        d.g.entered.load(Ordering::Acquire),
        "the loop ran while the contributor waited"
    );
    assert!(d.e.check_invariants().is_empty());
}
