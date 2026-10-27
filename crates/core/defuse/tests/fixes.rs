//! Reads taken back and relations not shared: what a client says of its
//! own reads and of its operation's nodes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, OpId, Operation, Report, Stop, ValueId};

/// `[s, x]`: contributes x to s.
struct Contrib;

impl Operation for Contrib {
    fn name(&self) -> &'static str {
        "contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.contribute(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[s]`: folds s and the next contribution to it, then takes both reads
/// back: its output does not depend on them.
struct Unread(AtomicU64);

impl Operation for Unread {
    fn name(&self) -> &'static str {
        "unread"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let s = Field(cx.args()[0]);
        cx.fold(s, None, None)?;
        cx.next_contribution(s, None)?;
        cx.unread(s);
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(ValueId::UNIT)
    }
}

/// `[f, k]`: f := f + 1 as a relation where k is 0, else defines nothing:
/// each node its own text.
struct Text;

impl Operation for Text {
    fn name(&self) -> &'static str {
        "text"
    }

    fn alike(&self) -> bool {
        false
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        if cx.values().as_int(cx.args()[1]) == Some(0) {
            cx.define_rel(Field(cx.args()[0]), 1);
        }
        Ok(ValueId::UNIT)
    }
}

/// `[f]`: the definition of f reaching it.
struct Get;

impl Operation for Get {
    fn name(&self) -> &'static str {
        "get"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
    }
}

#[test]
fn a_fold_and_a_next_taken_back_are_not_asked_again() {
    let e = Engine::new();
    let v = e.values();
    let contrib = e.register(Arc::new(Contrib));
    let unread = Arc::new(Unread(AtomicU64::new(0)));
    let u_op: OpId = e.register(unread.clone());
    let s = v.field("s");
    let u = e.new_unit();
    let c = e.push(u, contrib, vec![s.0, v.int(1)]);
    e.push(u, u_op, vec![s.0]);
    e.evaluate(2).unwrap();
    assert_eq!(unread.0.load(Ordering::Relaxed), 1);
    e.replace(c, contrib, vec![s.0, v.int(2)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(Report::by_op(&r.runs_by_op, "unread"), 0, "its reads of s were taken back: {r:?}");
    assert_eq!(unread.0.load(Ordering::Relaxed), 1);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn an_operation_whose_nodes_differ_shapes_none() {
    let e = Engine::new();
    let v = e.values();
    let text = e.register(Arc::new(Text));
    let get = e.register(Arc::new(Get));
    let f = v.field("c");
    let u = e.new_unit();
    let mut gets = Vec::new();
    for i in 0..50 {
        e.push(u, text, vec![f.0, v.int(i64::from(i != 0))]);
        gets.push(e.push(u, get, vec![f.0]));
    }
    let r = e.evaluate_sequential().unwrap();
    assert_eq!(r.shaped, 0, "one node's relation is no other's: {r:?}");
    for g in gets {
        assert_eq!(e.output(g).and_then(|x| v.as_int(x)), Some(1));
    }
    assert!(e.check_invariants().is_empty());
}

/// The interleaving of `SlowClaim`'s two runs: the later node claimed,
/// then the earlier one took it.
static CLAIMED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static TAKEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn wait_for(flag: &std::sync::atomic::AtomicU32, ms: u64) -> bool {
    let t = std::time::Instant::now();
    while flag.load(Ordering::SeqCst) == 0 {
        if t.elapsed() > std::time::Duration::from_millis(ms) {
            return false;
        }
        std::thread::yield_now();
    }
    true
}

/// `[n, role]`: claims its next n siblings; role 1 (the earlier node)
/// waits for role 2's claim first, role 2 then waits to be taken.
struct SlowClaim;

impl Operation for SlowClaim {
    fn name(&self) -> &'static str {
        "slow_claim"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let n = usize::try_from(v.as_int(cx.args()[0]).unwrap_or(0)).unwrap_or(0);
        let role = v.as_int(cx.args()[1]).unwrap_or(0);
        if role == 1 {
            wait_for(&CLAIMED, 500);
            cx.claim(n);
            TAKEN.store(1, Ordering::SeqCst);
        } else {
            cx.claim(n);
            CLAIMED.store(1, Ordering::SeqCst);
            wait_for(&TAKEN, 500);
        }
        Ok(cx.args()[0])
    }
}

/// `[us]`: spins.
struct Spin;

impl Operation for Spin {
    fn name(&self) -> &'static str {
        "spin"
    }

    fn sibling(&self) -> bool {
        false
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let t = std::time::Instant::now();
        let us = cx.values().as_int(cx.args()[0]).unwrap_or(0).unsigned_abs();
        while t.elapsed() < std::time::Duration::from_micros(us) {
            std::hint::spin_loop();
        }
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_node_taken_while_its_run_claims_gives_back_what_it_took() {
    let mut interleaved = 0;
    for _ in 0..20 {
        CLAIMED.store(0, Ordering::SeqCst);
        TAKEN.store(0, Ordering::SeqCst);
        let e = Engine::new();
        let v = e.values();
        let claim = e.register(Arc::new(SlowClaim));
        let get = e.register(Arc::new(Get));
        let spin = e.register(Arc::new(Spin));
        let f = v.field("x");
        let u = e.new_unit();
        // (work around them, so the workers start, and the two in two
        // workers' first shares: they run side by side)
        for _ in 0..8 {
            e.push(u, spin, vec![v.int(500)]);
        }
        e.push(u, claim, vec![v.int(1), v.int(1)]);
        let a = e.push(u, claim, vec![v.int(3), v.int(2)]);
        let sibs: Vec<_> = (0..3).map(|_| e.push(u, get, vec![f.0])).collect();
        for _ in 0..59 {
            e.push(u, spin, vec![v.int(500)]);
        }
        e.evaluate(8).unwrap();
        if CLAIMED.load(Ordering::SeqCst) == 1 && TAKEN.load(Ordering::SeqCst) == 1 {
            interleaved += 1;
        }
        assert_eq!(e.node_info(a).map(|i| i.state), Some(phitex_defuse::NodeState::Inert));
        for s in sibs {
            assert_eq!(e.node_info(s).map(|i| i.state), Some(phitex_defuse::NodeState::Done), "{s:?} is held by a node taken");
        }
        assert!(e.check_invariants().is_empty());
    }
    assert!(interleaved > 0, "the two claims never ran side by side");
}

/// `[s, to]`: a view on the fold of s up to node `to`, read one step in;
/// counts its runs.
struct ViewTo(std::sync::atomic::AtomicU64);

impl Operation for ViewTo {
    fn name(&self) -> &'static str {
        "view_to"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let v = cx.values();
        let to = phitex_defuse::NodeId::from_raw(v.as_int(cx.args()[1]).unwrap_or(0).unsigned_abs());
        let w = cx.view_through(Field(cx.args()[0]), None, to)?;
        Ok(cx.at(w, phitex_defuse::Step::Index(0)).map_or(ValueId::UNIT, |x| cx.whole(x)))
    }
}

#[test]
fn a_view_whose_bound_is_removed_runs_once_more_and_settles() {
    let e = Engine::new();
    let v = e.values();
    let contrib = e.register(Arc::new(Contrib));
    let view = Arc::new(ViewTo(AtomicU64::new(0)));
    let vo = e.register(view.clone());
    let s = v.field("s");
    let u = e.new_unit();
    let c = e.push(u, contrib, vec![s.0, v.seq_unit(v.int(1))]);
    e.push(u, vo, vec![s.0, v.int(i64::try_from(c.raw()).unwrap())]);
    e.evaluate(2).unwrap();
    assert_eq!(view.0.load(Ordering::Relaxed), 1);
    e.remove(c);
    let mut lim = phitex_defuse::Limits::default();
    lim.runs_per_node = 50;
    e.set_limits(lim);
    let r = e.evaluate(2);
    assert!(r.is_ok(), "the reader ran on for ever: {r:?}");
    assert_eq!(view.0.load(Ordering::Relaxed), 2, "once more, on the bound gone");
    assert!(e.check_invariants().is_empty());
}

/// The store's footprint counts what it interned, once per distinct value.
#[test]
fn the_value_store_footprint_counts_its_values() {
    let v = phitex_defuse::Values::new();
    let (n0, b0, _) = v.footprint();
    let a = v.str("abcdef");
    let l = v.list(vec![a, a, a]);
    assert_eq!(v.list(vec![a, a, a]), l);
    let (n1, b1, _) = v.footprint();
    assert_eq!(n1, n0 + 2, "a string and a list");
    assert_eq!(b1, b0 + (32 + 6) + (32 + 24));
}
