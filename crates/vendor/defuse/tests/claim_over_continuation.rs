//! A re-run that claims siblings its old continuation held keeps them:
//! siblings a, s1, s2, s3; `a` reads the environment's `mode`. In mode
//! cut, `a` claims s1 and follows `r`, which claims s2, s3; in mode whole,
//! `a` claims all three and follows nothing. No `s` runs as a node of its
//! own in either mode, before or after the edit, in every schedule; the
//! edit equals a cold build.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use defuse::{Claim, Ctx, Engine, Field, Locality, OpId, Operation, Report, Stop, ValueId, Values};

struct Ops {
    me: std::sync::OnceLock<OpId>,
    s_runs: std::sync::atomic::AtomicU64,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn claim(&self, a: &[ValueId], v: &Values, _f: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        match tag(v, a).as_str() {
            "a" | "r" => Claim::Pending,
            _ => Claim::None,
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "a" => {
                let cut = cx.use_field(v.field("mode"))? == Some(v.str("cut"));
                if cut {
                    cx.claim(1);
                    cx.follow(*self.me.get().unwrap(), vec![v.str("r")]);
                } else {
                    cx.claim(3);
                }
                Ok(v.int(if cut { 1 } else { 3 }))
            }
            "r" => {
                cx.claim(2);
                Ok(v.int(2))
            }
            _ => {
                self.s_runs.fetch_add(1, Relaxed);
                Ok(v.int(100))
            }
        }
    }
}
struct Threads;

impl defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        4
    }
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> defuse::RunResult + Sync),
        out: &mut Vec<defuse::RunResult>,
    ) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<std::sync::OnceLock<defuse::RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
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
        out.extend(slots.into_iter().map(|s| s.into_inner().expect("every job ran")));
    }
}

fn eval(eng: &mut Engine, i: usize) -> Report {
    match i % 4 {
        0 => eng.evaluate_sequential(),
        1 => eng.evaluate(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
            &mut defuse::Sequential,
        ),
        2 => eng.evaluate(
            &mut defuse::RandomScheduler::new(i as u64, true),
            &mut defuse::Sequential,
        ),
        _ => eng.evaluate(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
            &mut Threads,
        ),
    }
    .expect("evaluate")
}

fn build(mode: &str) -> (Engine, Vec<defuse::NodeId>, defuse::UnitId, Arc<Ops>) {
    let ops = Arc::new(Ops {
        me: std::sync::OnceLock::new(),
        s_runs: std::sync::atomic::AtomicU64::new(0),
    });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    let _ = ops.me.set(op);
    let u = eng.new_unit();
    let (m, mv) = (eng.values().field("mode"), eng.values().str(mode));
    eng.set_env(u, m, mv);
    let s = |e: &Engine, t: &str| e.values().str(t);
    let an = eng.push(u, op, vec![s(&eng, "a")]);
    let mut ss: Vec<_> = (0..3).map(|_| eng.push(u, op, vec![s(&eng, "s")])).collect();
    ss.push(an);
    (eng, ss, u, ops)
}

/// The `s` siblings that ran as nodes of their own (have an output);
/// `ss` ends with `a`.
fn own(eng: &Engine, ss: &[defuse::NodeId]) -> usize {
    ss[..3].iter().filter(|&&n| eng.output(n).is_some()).count()
}

#[test]
fn a_rerun_keeps_the_siblings_its_old_continuation_held() {
    for i in 0..80 {
        let (mut eng, ss, u, ops) = build("cut");
        eval(&mut eng, i);
        assert_eq!(own(&eng, &ss), 0, "cut {i}");
        let (m, w) = (eng.values().field("mode"), eng.values().str("whole"));
        eng.set_env(u, m, w);
        let before = ops.s_runs.load(Relaxed);
        eval(&mut eng, i + 1);
        if (i + 1) % 4 == 0 {
            assert_eq!(ops.s_runs.load(Relaxed), before, "no s runs on its own (seq) {i}");
        }
        assert_eq!(own(&eng, &ss), 0, "whole after cut {i}");
        let (mut cold, css, _, _) = build("whole");
        eval(&mut cold, 0);
        assert_eq!(own(&cold, &css), 0);
        // And back.
        let c = eng.values().str("cut");
        eng.set_env(u, m, c);
        eval(&mut eng, i + 2);
        assert_eq!(own(&eng, &ss), 0, "cut after whole {i}");
    }
}

/// Threaded cold builds: the continuation `r` runs while `a`'s claim of
/// `s1` waits on `s1`'s run (a doom mark); `r` must not count `s1`.
#[test]
fn a_continuation_skips_a_sibling_taken_while_it_runs() {
    for k in 0..1500 {
        let (mut eng, ss, _, _) = build("cut");
        let r = eval(&mut eng, 3);
        let owned: Vec<usize> = (0..3).filter(|&i| eng.output(ss[i]).is_some()).collect();
        if !owned.is_empty() {
            for &n in &ss {
                eprintln!("{:?}", eng.node_info(n));
            }
            panic!("k {k}: s {owned:?} ran on their own; {r:?}");
        }
    }
}
