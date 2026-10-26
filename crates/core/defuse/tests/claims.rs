//! Claims among placed nodes and loop-back re-asks, under threads: the
//! interleavings a client's text pieces meet (a piece read on by its
//! siblings, a continuation placed among them, a file read back last).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{At, Ctx, Engine, Field, FieldAt, NodeId, NodeState, OpId, Operation, Stop, ValueId};

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

/// `[f, v, ms]`: spins `ms` milliseconds, then defines f = v.
struct SlowDef;

impl Operation for SlowDef {
    fn name(&self) -> &'static str {
        "slow_def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, x) = (Field(cx.args()[0]), cx.args()[1]);
        let ms = cx.values().as_int(cx.args()[2]).unwrap() as u64;
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            std::hint::spin_loop();
        }
        cx.define(f, x);
        Ok(x)
    }
}

/// `[x]`: outputs x.
struct Text;

impl Operation for Text {
    fn name(&self) -> &'static str {
        "text"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx.args()[0])
    }
}

/// `[n]`: claims its next n siblings; outputs their first arguments.
struct Claimer;

impl Operation for Claimer {
    fn name(&self) -> &'static str {
        "claimer"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let n = cx.values().as_int(cx.args()[0]).unwrap() as usize;
        let mut xs = Vec::new();
        for i in 0..n {
            if let Some((_, args)) = cx.following(i) {
                xs.push(args[0]);
            }
        }
        cx.claim(n);
        Ok(cx.values().list(xs))
    }
}

/// `[n, k]`: places a `Peek` child, and claims its next n siblings when
/// `k` is 1 (none when 0).
struct ClaimPlace(std::sync::OnceLock<OpId>);

impl Operation for ClaimPlace {
    fn name(&self) -> &'static str {
        "claim_place"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let n = v.as_int(cx.args()[0]).unwrap() as usize;
        let on = v.as_int(cx.args()[1]).unwrap() != 0;
        let peek = *self.0.get().expect("registered");
        cx.child(peek, vec![v.int(0)]);
        if on {
            for i in 0..n {
                let _ = cx.following(i);
            }
            cx.claim(n);
        }
        Ok(ValueId::UNIT)
    }
}

/// `[_]`: outputs its next sibling's first argument (UNIT if none),
/// claiming nothing: a continuation reading on.
struct Peek;

impl Operation for Peek {
    fn name(&self) -> &'static str {
        "peek"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx.following(0).map_or(ValueId::UNIT, |(_, a)| a[0]))
    }
}

/// `[f]`: outputs f's last definition on the line; counts its runs.
struct FinalGet(Arc<AtomicU64>);

impl Operation for FinalGet {
    fn name(&self) -> &'static str {
        "final_get"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let f = Field(cx.args()[0]);
        Ok(match cx.read_at(f, At::Final)? {
            FieldAt::Def(x, _) => x,
            _ => ValueId::UNIT,
        })
    }
}

struct Ops {
    def: OpId,
    slow_def: OpId,
    text: OpId,
    claimer: OpId,
    claim_place: OpId,
    final_get: OpId,
    final_runs: Arc<AtomicU64>,
}

fn engine() -> (Engine, Ops) {
    let e = Engine::new();
    let cp = Arc::new(ClaimPlace(std::sync::OnceLock::new()));
    let final_runs = Arc::new(AtomicU64::new(0));
    let ops = Ops {
        def: e.register(Arc::new(Def)),
        slow_def: e.register(Arc::new(SlowDef)),
        text: e.register(Arc::new(Text)),
        claimer: e.register(Arc::new(Claimer)),
        claim_place: e.register(cp.clone()),
        final_get: e.register(Arc::new(FinalGet(final_runs.clone()))),
        final_runs,
    };
    let _ = cp.0.set(e.register(Arc::new(Peek)));
    (e, ops)
}

fn op_nodes(e: &Engine, u: phitex_defuse::UnitId, op: &str) -> Vec<NodeId> {
    e.order(u)
        .into_iter()
        .filter(|&n| e.node_info(n).is_some_and(|i| i.op == op))
        .collect()
}

#[test]
fn a_sibling_a_later_claim_holds_is_taken_with_it() {
    // B holds C and D; A, made to claim three, takes B and with it what B
    // held, in order, not passing over C and D to E.
    let (e, o) = engine();
    let v = e.values();
    let u = e.new_unit();
    let a = e.push(u, o.claimer, vec![v.int(0)]);
    let b = e.push(u, o.claimer, vec![v.int(2)]);
    let c = e.push(u, o.text, vec![v.str("c")]);
    let d = e.push(u, o.text, vec![v.str("d")]);
    let x = e.push(u, o.text, vec![v.str("e")]);
    e.evaluate(2).unwrap();
    assert_eq!(e.claimed(b), vec![c, d]);
    e.replace(a, o.claimer, vec![v.int(3)]);
    e.evaluate(4).unwrap();
    let out = v.with_list(e.output(a).unwrap(), <[ValueId]>::to_vec).unwrap();
    assert_eq!(out, vec![v.int(2), v.str("c"), v.str("d")]);
    assert_eq!(e.claimed(a), vec![b, c, d]);
    for n in [b, c, d] {
        assert_eq!(e.node_info(n).unwrap().state, NodeState::Inert);
        assert_eq!(e.node_info(n).unwrap().claimer, Some(a));
    }
    assert_eq!(e.node_info(x).unwrap().state, NodeState::Done);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_sibling_released_re_asks_the_node_that_passed_it() {
    // P holds S and places K between them; K reads on past S (an earlier
    // claim holds it) to T. P made to claim nothing gives S back, and K,
    // whose answer moved though nothing it read was defined, reads S.
    for threads in [1, 4] {
        let (e, o) = engine();
        let v = e.values();
        let u = e.new_unit();
        let p = e.push(u, o.claim_place, vec![v.int(1), v.int(1)]);
        let s = e.push(u, o.text, vec![v.str("s")]);
        e.push(u, o.text, vec![v.str("t")]);
        e.evaluate(threads).unwrap();
        let k = op_nodes(&e, u, "peek")[0];
        assert_eq!(e.output(k), Some(v.str("t")), "-j{threads}");
        assert_eq!(e.node_info(s).unwrap().claimer, Some(p));
        e.replace(p, o.claim_place, vec![v.int(1), v.int(0)]);
        e.evaluate(threads).unwrap();
        let k = op_nodes(&e, u, "peek")[0];
        assert_eq!(e.output(k), Some(v.str("s")), "-j{threads}");
        assert_eq!(e.node_info(s).unwrap().state, NodeState::Done);
        assert!(e.check_invariants().is_empty());
    }
}

#[test]
fn a_removed_claimer_gives_back_what_it_held() {
    // C holds S and T; C removed, both run, held by no one.
    let (e, o) = engine();
    let v = e.values();
    let u = e.new_unit();
    let c = e.push(u, o.claimer, vec![v.int(2)]);
    let s = e.push(u, o.text, vec![v.str("s")]);
    let t = e.push(u, o.text, vec![v.str("t")]);
    e.evaluate(2).unwrap();
    assert_eq!(e.claimed(c), vec![s, t]);
    e.remove(c);
    e.evaluate(2).unwrap();
    for n in [s, t] {
        let i = e.node_info(n).unwrap();
        assert_eq!((i.state, i.claimer), (NodeState::Done, None));
    }
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_loopback_reader_runs_again_once_the_line_is_quiet() {
    // R reads f at its last definition; W1 defines it at once, W2 after a
    // long run. R re-asked as W1 lands is held while W2 runs, not run on a
    // value about to move, and the evaluation does not end over it: R
    // runs at most twice and ends with W2's value.
    let threads = std::thread::available_parallelism().map_or(1, usize::from).clamp(2, 8);
    let (e, o) = engine();
    let v = e.values();
    let fv = v.field("f").0;
    let u = e.new_unit();
    let r = e.push(u, o.final_get, vec![fv]);
    e.push(u, o.def, vec![fv, v.int(1)]);
    e.push(u, o.slow_def, vec![fv, v.int(2), v.int(40)]);
    let rep = e.evaluate(threads).unwrap();
    assert_eq!(e.output(r), Some(v.int(2)));
    assert!(o.final_runs.load(Ordering::Relaxed) <= 2, "{} runs: {rep:?}", o.final_runs.load(Ordering::Relaxed));
    assert!(rep.capped.is_empty());
    assert!(e.check_invariants().is_empty());
}
