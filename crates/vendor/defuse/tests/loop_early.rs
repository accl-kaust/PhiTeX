//! Early loop-back fields (`declare_loopback_early`): a field takes the
//! value published for it at the publisher's commit, not at the round's
//! end. Unit [p, fw, r]: `p` reads `n` and publishes `step(n)`; `fw`
//! forwards `n` into `out`; `r` reads `n` and `out`.

use std::sync::Arc;

use defuse::{CapPolicy, Ctx, Engine, Field, Locality, Operation, Report, Stop, ValueId, Values};

struct Ops {
    // `p` publishes `n + 1` up to 3; else `1 - n` (no fixed point).
    flip: bool,
    // `p` declares `n` itself (early when `early`).
    late: bool,
    early: bool,
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if v.as_str(a[0]).as_deref() == Some("fw") { vec![(v.field("out"), Locality::Local)] } else { Vec::new() })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let (n, out) = (v.field("n"), v.field("out"));
        match v.as_str(a[0]).as_deref() {
            Some("p") => {
                if self.late && self.early {
                    cx.declare_loopback_early(n, v.int(0));
                } else if self.late {
                    cx.declare_loopback(n, v.int(0));
                }
                let x = cx.use_field(n)?.and_then(|y| v.as_int(y)).unwrap_or(0);
                cx.publish(n, v.int(if self.flip { 1 - x } else { (x + 1).min(3) }));
                Ok(v.int(x))
            }
            Some("fw") => {
                cx.define_forward(out, n, Locality::Local)?;
                Ok(ValueId::UNIT)
            }
            _ => {
                let x = cx.use_field(n)?.and_then(|y| v.as_int(y)).unwrap_or(-1);
                let y = cx.use_field(out)?.and_then(|y| v.as_int(y)).unwrap_or(-1);
                Ok(v.int(x * 10 + y))
            }
        }
    }
}

fn run(flip: bool, late: bool, early: bool, threads: bool) -> (Option<i64>, Result<Report, String>) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops { flip, late, early }));
    let (n, zero) = (eng.values().field("n"), eng.values().int(0));
    if flip {
        eng.set_max_rounds(5);
        eng.set_round_cap_policy(CapPolicy::Finalize);
    }
    if !late {
        if early {
            eng.declare_loopback_early(n, zero);
        } else {
            eng.declare_loopback(n, zero);
        }
    }
    let u = eng.new_unit();
    for s in ["p", "fw", "r"] {
        let a = vec![eng.values().str(s)];
        eng.push(u, op, a);
    }
    let r = eng.push(u, op, vec![eng.values().str("r")]);
    let rep = if threads {
        eng.evaluate(&mut defuse::Greedy::default(), &mut defuse::Sequential)
    } else {
        eng.evaluate_sequential()
    };
    (eng.output(r).and_then(|y| eng.values().as_int(y)), rep.map_err(|e| e.to_string()))
}

#[test]
fn an_early_field_converges_to_the_rounds_result_in_one_round() {
    for late in [false, true] {
        let (exact, er) = run(false, late, false, false);
        let (early, rr) = run(false, late, true, false);
        let (er, rr) = (er.unwrap(), rr.unwrap());
        assert_eq!(exact, Some(33));
        assert_eq!(early, exact, "late {late}");
        assert!(er.early_loops.is_empty());
        assert!(rr.rounds < er.rounds, "late {late}: rounds {} against {}", rr.rounds, er.rounds);
        assert_eq!(rr.early_loops.len(), 1, "{:?}", rr.early_loops);
        assert_eq!(rr.early_loops[0].changes, 3);
        assert!(!rr.early_loops[0].capped);
        // (0 -> 1 -> 2 -> 3: changed twice after its first publication.)
        assert!(rr.early_may_differ());
    }
}

#[test]
fn an_oscillating_early_field_is_capped_and_reported() {
    let (_, rr) = run(true, false, true, false);
    let rr = rr.unwrap();
    assert_eq!(rr.early_loops.len(), 1, "{:?}", rr.early_loops);
    assert!(rr.early_loops[0].capped);
    assert_eq!(rr.early_loops[0].changes, 5);
    assert!(rr.capped.is_some());
    assert!(rr.early_may_differ());
}

#[test]
fn an_oscillating_early_field_under_the_error_policy_fails() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops { flip: true, late: false, early: true }));
    let (n, zero) = (eng.values().field("n"), eng.values().int(0));
    eng.set_max_rounds(5);
    eng.declare_loopback_early(n, zero);
    let u = eng.new_unit();
    let a = vec![eng.values().str("p")];
    eng.push(u, op, a);
    assert!(eng.evaluate_sequential().is_err());
}

#[test]
fn a_one_change_field_does_not_warn() {
    // (`p` publishes 1 from 0, then 2 from 1: stop at 1 with `min(1)`.)
    struct One;
    impl Operation for One {
        fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
            Some(Vec::new())
        }
        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            let v = cx.values();
            let n = v.field("n");
            let x = cx.use_field(n)?.and_then(|y| v.as_int(y)).unwrap_or(0);
            if v.as_str(cx.args()[0]).as_deref() == Some("p") {
                cx.publish(n, v.int(1));
            }
            Ok(v.int(x))
        }
    }
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(One));
    let (n, zero) = (eng.values().field("n"), eng.values().int(0));
    eng.declare_loopback_early(n, zero);
    let u = eng.new_unit();
    let a = vec![eng.values().str("p")];
    eng.push(u, op, a);
    let r = eng.push(u, op, vec![eng.values().str("r")]);
    let rep = eng.evaluate_sequential().unwrap();
    assert_eq!(eng.output(r).and_then(|y| eng.values().as_int(y)), Some(1));
    assert_eq!(rep.early_loops.len(), 1);
    assert_eq!(rep.early_loops[0].changes, 1);
    assert!(!rep.early_may_differ());
}

/// An early field moves when its publication is final for the round (its
/// publisher committed, nothing before it in work): a reader after the
/// publisher runs once per real change, however the schedule ran the
/// publisher ahead. Unit [d, d2, p, r]: `d` and `d2` define `y` and `z`,
/// `p` publishes `n := y + z`, `r` reads `n`. An edit of both `d` and
/// `d2`, under random schedules that speculate.
#[test]
fn a_reader_after_the_publisher_runs_once_per_change() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Final {
        r_runs: Arc<AtomicUsize>,
    }
    impl Operation for Final {
        fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
            Some(match v.as_str(a[0]).as_deref() {
                Some("d") => vec![(v.field("y"), Locality::Local)],
                Some("d2") => vec![(v.field("z"), Locality::Local)],
                _ => Vec::new(),
            })
        }
        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            let v = cx.values();
            let a = cx.args();
            match v.as_str(a[0]).as_deref() {
                Some("d") => {
                    cx.define(v.field("y"), a[1]);
                    Ok(ValueId::UNIT)
                }
                Some("d2") => {
                    cx.define(v.field("z"), a[1]);
                    Ok(ValueId::UNIT)
                }
                Some("p") => {
                    let y = cx.use_field(v.field("y"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                    let z = cx.use_field(v.field("z"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                    cx.publish(v.field("n"), v.int(y + z));
                    Ok(ValueId::UNIT)
                }
                _ => {
                    self.r_runs.fetch_add(1, Ordering::Relaxed);
                    Ok(cx.use_field(v.field("n"))?.unwrap_or(ValueId::UNIT))
                }
            }
        }
    }
    for seed in 0..64 {
        let runs = Arc::new(AtomicUsize::new(0));
        let mut eng = Engine::new();
        let op = eng.register(Arc::new(Final { r_runs: runs.clone() }));
        let (n, zero) = (eng.values().field("n"), eng.values().int(0));
        eng.declare_loopback_early(n, zero);
        let u = eng.new_unit();
        let da = vec![eng.values().str("d"), eng.values().int(1)];
        let d = eng.push(u, op, da);
        let da2 = vec![eng.values().str("d2"), eng.values().int(10)];
        let d2 = eng.push(u, op, da2);
        let p = vec![eng.values().str("p")];
        eng.push(u, op, p);
        let r = vec![eng.values().str("r")];
        let r = eng.push(u, op, r);
        eng.evaluate_sequential().unwrap();
        assert_eq!(eng.output(r).and_then(|x| eng.values().as_int(x)), Some(11));
        let before = runs.load(Ordering::Relaxed);
        let da = vec![eng.values().str("d"), eng.values().int(2)];
        eng.replace(d, op, da);
        let da2 = vec![eng.values().str("d2"), eng.values().int(20)];
        eng.replace(d2, op, da2);
        eng.evaluate(&mut defuse::RandomScheduler::new(seed, true), &mut defuse::Sequential).unwrap();
        assert_eq!(eng.output(r).and_then(|x| eng.values().as_int(x)), Some(22), "seed {seed}");
        assert_eq!(runs.load(Ordering::Relaxed) - before, 1, "seed {seed}: the reader ran more than once");
    }
}
