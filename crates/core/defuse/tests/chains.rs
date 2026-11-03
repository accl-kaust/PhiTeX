//! `Operation::chains`: a node of a chain waits for the one before it that
//! moved what the chain read, so a value moving down the chain costs about
//! one run a node at any number of threads; nodes nothing moves run side by
//! side. The order of the runs only: what they make is what they read.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{Ctx, Engine, Field, Operation, Stop, ValueId};

fn spin(d: Duration) {
    let t = Instant::now();
    while t.elapsed() < d {
        std::hint::spin_loop();
    }
}

/// `[f, x]`: defines f := x.
struct Root;

impl Operation for Root {
    fn name(&self) -> &'static str {
        "root"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[root, before, out]`: a page's routine: out := root + before, the
/// field the node before it left.
struct Link(AtomicU64);

impl Operation for Link {
    fn name(&self) -> &'static str {
        "link"
    }

    fn chains(&self) -> bool {
        true
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let r = cx.read(Field(cx.args()[0]))?;
        let b = cx.read(Field(cx.args()[1]))?;
        spin(Duration::from_millis(2));
        let v = cx.values();
        let int = |x: Option<ValueId>| x.and_then(|x| v.as_int(x)).unwrap_or(0);
        let out = v.int(int(r) + int(b));
        cx.define(Field(cx.args()[2]), out);
        Ok(out)
    }
}

/// `[root, out]`: a page's routine that reads only what nothing of its
/// chain defines. Given 2, it waits for another run of its kind to begin
/// beside it, and counts when one did.
struct Side(AtomicU64, AtomicU64);

impl Operation for Side {
    fn name(&self) -> &'static str {
        "side"
    }

    fn chains(&self) -> bool {
        true
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let r = cx.read(Field(cx.args()[0]))?;
        if r.and_then(|r| cx.values().as_int(r)) == Some(2) {
            self.0.fetch_add(1, Ordering::SeqCst);
            let t = Instant::now();
            while self.0.load(Ordering::SeqCst) < 2 && t.elapsed() < Duration::from_secs(20) {
                std::hint::spin_loop();
            }
            if self.0.load(Ordering::SeqCst) >= 2 {
                self.1.fetch_add(1, Ordering::SeqCst);
            }
        }
        let out = r.unwrap_or(ValueId::UNIT);
        cx.define(Field(cx.args()[1]), out);
        Ok(out)
    }
}

const LINKS: i64 = 24;

/// A change the whole chain reads re-asks all of it at once. Once the first
/// edit showed what moves, each runs about once, not once per value on its
/// way (each of eight workers on the value before the last).
#[test]
fn a_value_moving_down_a_chain_costs_about_one_run_a_node() {
    let e = Engine::new();
    let v = e.values();
    let root = e.register(Arc::new(Root));
    let link = Arc::new(Link(AtomicU64::new(0)));
    let lop = e.register(link.clone());
    let u = e.new_unit();
    let r = v.field("root");
    let rn = e.push(u, root, vec![r.0, v.int(1)]);
    let mut before = r;
    let mut last = rn;
    for i in 0..LINKS {
        let out = v.field(&format!("x{i}"));
        last = e.push(u, lop, vec![r.0, before.0, out.0]);
        before = out;
    }
    e.evaluate(8).unwrap();
    for (k, x) in [2, 3, 4, 5].into_iter().enumerate() {
        e.replace(rn, root, vec![r.0, v.int(x)]);
        let n0 = link.0.load(Ordering::Relaxed);
        e.evaluate(8).unwrap();
        let runs = link.0.load(Ordering::Relaxed) - n0;
        assert_eq!(e.output(last), Some(v.int(x * (LINKS + 1))));
        if k > 0 {
            assert!(runs < 2 * LINKS as u64, "edit {k}: {runs} runs of {LINKS} nodes");
        }
    }
    assert!(e.check_invariants().is_empty());
}

/// Nodes of a chain none of which moved another's read run side by side.
#[test]
fn nodes_of_a_chain_nothing_moves_run_side_by_side() {
    let e = Engine::new();
    let v = e.values();
    let root = e.register(Arc::new(Root));
    let side = Arc::new(Side(AtomicU64::new(0), AtomicU64::new(0)));
    let sop = e.register(side.clone());
    let u = e.new_unit();
    let r = v.field("root");
    let rn = e.push(u, root, vec![r.0, v.int(1)]);
    for i in 0..4 {
        e.push(u, sop, vec![r.0, v.field(&format!("x{i}")).0]);
    }
    e.evaluate(4).unwrap();
    e.replace(rn, root, vec![r.0, v.int(2)]);
    e.evaluate(4).unwrap();
    assert!(side.1.load(Ordering::SeqCst) >= 2, "ran one after another");
    assert!(e.check_invariants().is_empty());
}
