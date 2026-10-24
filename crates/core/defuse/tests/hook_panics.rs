//! A panic in an operation's static hook never unwinds out of the engine:
//! the evaluation fails with `EvalError::HookPanicked`, naming the hook,
//! whether the hook ran at `push` (reported by the next evaluation) or for
//! a node a run made; the panic is reported once, and the engine goes on
//! once the text no longer panics.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use phitex_defuse::{Ctx, Effect, Engine, EvalError, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        assert!(tag(v, a) != "bad-defs", "defs blew up");
        Some(vec![(v.field("x"), Locality::Local)])
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> Effect {
        assert!(tag(v, a) != "bad-effect", "effect blew up");
        Effect::None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) == "maker" {
            cx.child(ONE.get().copied().unwrap(), vec![v.str("bad-effect")]);
        }
        cx.define(v.field("x"), v.int(1));
        Ok(ValueId::UNIT)
    }
}

static ONE: std::sync::OnceLock<phitex_defuse::OpId> = std::sync::OnceLock::new();

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
                        let i = next.fetch_add(1, Relaxed);
                        if i >= n {
                            break;
                        }
                        let _ = slots[i].set(run(i));
                    }
                });
            }
        });
        out.extend(slots.into_iter().map(|s| s.into_inner().expect("ran")));
    }
}

fn hook_of(e: &EvalError) -> Option<&'static str> {
    match e {
        EvalError::HookPanicked { hook, .. } => Some(hook),
        _ => None,
    }
}

#[test]
fn a_hook_panic_at_push_fails_the_next_evaluation_once() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let ok = eng.values().str("ok");
    eng.push(u, op, vec![ok]);
    let bad = eng.values().str("bad-defs");
    let n = eng.push(u, op, vec![bad]);
    let e = eng.evaluate_sequential().expect_err("the hook panicked");
    assert_eq!(hook_of(&e), Some("defs"), "{e:?}");
    // Its text fixed, the engine goes on.
    eng.replace(n, op, vec![ok]);
    eng.evaluate_sequential().expect("no panic now");
    assert!(eng.check_reads().is_empty());
}

#[test]
fn a_hook_panic_for_a_made_node_fails_the_evaluation() {
    for parallel in [false, true] {
        let mut eng = Engine::new();
        let op = eng.register(Arc::new(Ops));
        let _ = ONE.set(op);
        let u = eng.new_unit();
        let maker = eng.values().str("maker");
        let m = eng.push(u, op, vec![maker]);
        let r = if parallel {
            eng.evaluate(&mut phitex_defuse::Greedy::default(), &mut Threads)
        } else {
            eng.evaluate_sequential()
        };
        let e = r.expect_err("the hook panicked");
        assert_eq!(hook_of(&e), Some("effect"), "{e:?}");
        // Reported once: the maker's text changed, the next one is clean.
        let ok = eng.values().str("ok");
        eng.replace(m, op, vec![ok]);
        eng.evaluate_sequential().expect("no panic now");
    }
}
