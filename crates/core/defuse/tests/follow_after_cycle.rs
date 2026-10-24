//! A placer's follow nodes between a node and its own continuation
//! (PhiTeX's block step, r5-blk): top nodes `q`, `e`, `s`. `q` places its
//! continuation `r`; `s` places `f0`, `f1` after `q` (`follow_after`), so
//! they sit between `q` and `r`. An `f` declares its contribution to `g`
//! as 0 and contributes 1. `r` folds `g` from the start, defines `h` and
//! contributes 1 to `a`; `e` reads `h` and contributes 1 to `a`; `s` folds
//! `a`. The evaluation ends with each node run a bounded number of times,
//! as a cold sequential build gives.

use std::sync::Arc;
use std::sync::Mutex;

use phitex_defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Stop, ValueId, Values};

struct Ops {
    q: Mutex<Option<NodeId>>,
    runs: Mutex<Vec<String>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "r" { vec![(v.field("h"), Locality::Global)] } else { Vec::new() })
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        match tag(v, a).as_str() {
            "f" => Some(vec![(v.field("g"), v.int(0))]),
            "c" => Some(Vec::new()),
            _ => None,
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let t = tag(v, &a);
        let out = match t.as_str() {
            "q" => {
                let r = v.str("r");
                cx.follow(cx.op(), vec![r]);
                ValueId::UNIT
            }
            "r" => {
                let x = cx.acc_range(v.field("g"), None)?;
                cx.define(v.field("h"), x);
                cx.contribute(v.field("a"), v.int(1));
                x
            }
            "e" => {
                let x = cx.use_field(v.field("h"))?;
                cx.contribute(v.field("a"), v.int(1));
                x.unwrap_or(ValueId::UNIT)
            }
            "s" => {
                let x = cx.acc_range(v.field("a"), None)?;
                let q = self.q.lock().unwrap().expect("q");
                for i in 0..2 {
                    let f = vec![v.str("f"), v.int(i)];
                    cx.follow_after(q, cx.op(), f);
                }
                x
            }
            "c" => ValueId::UNIT,
            _ => {
                cx.contribute(v.field("g"), v.int(1));
                // (Its own continuation, as a scan places one.)
                let c = v.str("c");
                cx.follow(cx.op(), vec![c]);
                ValueId::UNIT
            }
        };
        self.runs.lock().unwrap().push(t);
        Ok(out)
    }
}

fn build() -> (Engine, Arc<Ops>, NodeId) {
    build_v(0).0
}

/// The program with `q`'s text at version `qv`; also `q` and `e` and the
/// operation.
fn build_v(qv: i64) -> ((Engine, Arc<Ops>, NodeId), NodeId, NodeId, phitex_defuse::OpId) {
    let mut eng = Engine::new();
    let ops = Arc::new(Ops {
        q: Mutex::new(None),
        runs: Mutex::new(Vec::new()),
    });
    let op = eng.register(ops.clone());
    for f in ["g", "a"] {
        let f = eng.values().field(f);
        eng.declare_accumulator(f, Arc::new(phitex_defuse::SumMonoid));
    }
    let u = eng.new_unit();
    let q = vec![eng.values().str("q"), eng.values().int(qv)];
    let q = eng.push(u, op, q);
    *ops.q.lock().unwrap() = Some(q);
    let e = vec![eng.values().str("e")];
    let e = eng.push(u, op, e);
    let s = vec![eng.values().str("s")];
    let s = eng.push(u, op, s);
    eng.add_root(s);
    eng.add_root(e);
    ((eng, ops, s), q, e, op)
}

#[test]
fn follow_nodes_before_a_continuation_converge() {
    let mut limits = phitex_defuse::Limits::default();
    limits.runs_per_node = 20;
    for sched in ["seq", "par"] {
        let (mut eng, ops, s) = build();
        eng.set_limits(limits.clone());
        let r = match sched {
            "seq" => eng.evaluate_sequential(),
            _ => eng.evaluate(&mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 256), &mut Threads),
        };
        let runs = ops.runs.lock().unwrap().clone();
        let r = r.unwrap_or_else(|e| panic!("{sched}: {e}\nruns {runs:?}"));
        // (f0, f1 contribute 2 to g; r reads it; e and r contribute 2 to a.)
        assert_eq!(eng.output(s).and_then(|x| eng.values().as_int(x)), Some(2), "{sched}: runs {runs:?}");
        assert!(r.runs <= 12, "{sched}: {} runs: {runs:?}", r.runs);
    }
}

/// `q` runs again (its text edited) and places its continuation again:
/// the follow nodes `s` placed after `q` stay before it, as a cold build
/// of the edited text has them, and nothing moves back and forth.
#[test]
fn a_continuation_placed_again_stays_after_the_follow_nodes() {
    let ((mut eng, ops, s), q, e, op) = build_v(0);
    eng.evaluate_sequential().expect("cold");
    let a = vec![eng.values().str("q"), eng.values().int(1)];
    eng.replace(q, op, a);
    ops.runs.lock().unwrap().clear();
    let mut limits = phitex_defuse::Limits::default();
    limits.runs_per_node = 20;
    eng.set_limits(limits);
    let r = eng.evaluate_sequential();
    let runs = ops.runs.lock().unwrap().clone();
    r.unwrap_or_else(|err| panic!("edit: {err}\nruns {runs:?}"));
    let ((mut cold, _, cs), _, ce, _) = build_v(1);
    cold.evaluate_sequential().expect("cold of the edit");
    let int = |g: &Engine, n: NodeId| g.output(n).and_then(|x| g.values().as_int(x));
    assert_eq!((int(&eng, e), int(&eng, s)), (int(&cold, ce), int(&cold, cs)), "runs {runs:?}");
    assert!(runs.len() <= 6, "runs {runs:?}");
}

struct Threads;

impl phitex_defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        4
    }
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> phitex_defuse::RunResult + Sync),
        out: &mut Vec<phitex_defuse::RunResult>,
    ) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<std::sync::OnceLock<phitex_defuse::RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        let _ = slots[i].set(run(i));
                    }
                });
            }
        });
        out.extend(slots.into_iter().map(|s| s.into_inner().expect("every job ran")));
    }
}

/// `["p", v]` places children `a` and `b`; `["s"]` places a follow node
/// after `a` (between `a` and `b`, the anchor its first argument once it
/// is known). Each run's tag is recorded.
struct Kids {
    a: Mutex<Option<NodeId>>,
    runs: Mutex<Vec<String>>,
}

impl Operation for Kids {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let t = tag(v, &a);
        match t.as_str() {
            "p" => {
                for k in ["a", "b"] {
                    let x = vec![v.str(k)];
                    cx.child(cx.op(), x);
                }
            }
            "s" => {
                if let Some(at) = *self.a.lock().unwrap() {
                    let f = vec![v.str("f")];
                    cx.follow_after(at, cx.op(), f);
                }
            }
            _ => {}
        }
        self.runs.lock().unwrap().push(t);
        Ok(ValueId::UNIT)
    }
}

/// A parent run again with the same children keeps them, though another
/// placer's follow node sits between two of them (PhiTeX: a toc's pieces
/// made again, one after another, on a heading edit).
#[test]
fn children_around_another_placers_follow_node_are_kept() {
    let mut eng = Engine::new();
    let ops = Arc::new(Kids {
        a: Mutex::new(None),
        runs: Mutex::new(Vec::new()),
    });
    let op = eng.register(ops.clone());
    let u = eng.new_unit();
    let pa = vec![eng.values().str("p"), eng.values().int(0)];
    let p = eng.push(u, op, pa);
    let sa = vec![eng.values().str("s")];
    let s = eng.push(u, op, sa.clone());
    eng.add_root(p);
    eng.add_root(s);
    eng.evaluate_sequential().expect("cold");
    let a = eng.children(p)[0];
    *ops.a.lock().unwrap() = Some(a);
    // (`s` runs again, now placing its follow node after `a`.)
    let sa2 = vec![eng.values().str("s"), eng.values().int(1)];
    eng.replace(s, op, sa2);
    eng.evaluate_sequential().expect("s places");
    ops.runs.lock().unwrap().clear();
    let pa2 = vec![eng.values().str("p"), eng.values().int(1)];
    eng.replace(p, op, pa2);
    eng.evaluate_sequential().expect("p again");
    let runs = ops.runs.lock().unwrap().clone();
    assert_eq!(runs, ["p"], "only p runs again");
}
