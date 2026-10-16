//! Static definition lists are guesses: `A` declares {f} and its run
//! defines f and g; `B` after it reads g. B sees A's g (not the
//! environment's), the report counts the refutation, and the result
//! equals a build where A declares {f, g}. A follow child `Q` of `P`
//! (P declares {a}) defining `b`: a reader after Q sees Q's b, whether Q
//! declares b or not. In every schedule.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use defuse::{Ctx, Engine, Field, Locality, OpId, Operation, Report, Stop, ValueId, Values};

struct Ops {
    me: std::sync::OnceLock<OpId>,
    /// A declares g too (the reference build).
    full: AtomicBool,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let l = |n: &str| (v.field(n), Locality::Local);
        Some(match tag(v, a).as_str() {
            "a" if self.full.load(Relaxed) => vec![l("f"), l("g")],
            "a" => vec![l("f")],
            "p" => vec![l("a")],
            "q" if self.full.load(Relaxed) => vec![l("b")],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let int = |x: Option<ValueId>| x.and_then(|x| v.as_int(x)).unwrap_or(-1);
        match tag(v, &a).as_str() {
            "a" => {
                cx.define(v.field("f"), v.int(1));
                cx.define(v.field("g"), v.int(2));
                Ok(ValueId::UNIT)
            }
            "p" => {
                cx.define(v.field("a"), v.int(3));
                cx.follow(*self.me.get().unwrap(), vec![v.str("q")]);
                Ok(ValueId::UNIT)
            }
            "q" => {
                cx.define(v.field("b"), v.int(4));
                Ok(ValueId::UNIT)
            }
            "rg" => Ok(v.int(int(cx.use_field(v.field("g"))?))),
            _ => Ok(v.int(int(cx.use_field(v.field("b"))?))),
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

fn build(full: bool) -> (Engine, Vec<defuse::NodeId>) {
    let ops = Arc::new(Ops {
        me: std::sync::OnceLock::new(),
        full: AtomicBool::new(full),
    });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    let _ = ops.me.set(op);
    let u = eng.new_unit();
    let (g, b, z) = (eng.values().field("g"), eng.values().field("b"), eng.values().int(0));
    eng.set_environment([(g, z), (b, z)]);
    let s = |e: &Engine, t: &str| e.values().str(t);
    eng.push(u, op, vec![s(&eng, "a")]);
    let rg = eng.push(u, op, vec![s(&eng, "rg")]);
    eng.push(u, op, vec![s(&eng, "p")]);
    let rb = eng.push(u, op, vec![s(&eng, "rb")]);
    (eng, vec![rg, rb])
}

fn outs(eng: &Engine, ns: &[defuse::NodeId]) -> Vec<Option<i64>> {
    ns.iter()
        .map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x)))
        .collect()
}

#[test]
fn a_definition_outside_the_static_list_reaches_its_readers() {
    let (mut want, wns) = build(true);
    eval(&mut want, 0);
    assert_eq!(outs(&want, &wns), vec![Some(2), Some(4)]);
    for i in 0..80 {
        let (mut eng, ns) = build(false);
        let rep = eval(&mut eng, i);
        assert_eq!(
            outs(&eng, &ns),
            outs(&want, &wns),
            "schedule {i}: {:?}",
            rep.rerun_causes
        );
        assert!(rep.defs_refuted >= 1, "{i}: {}", rep.defs_refuted);
    }
}
