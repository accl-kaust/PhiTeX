//! Predictions of nodes with a pending definition set (no `defs`, a
//! `may_define` bound): filed as definitions to come, a reader after such
//! a node takes its prediction instead of guessing past it, and the
//! node's run confirms or refutes it. Unit `[o?, d_0 .. d_N, c?, r]`: each
//! `d_i` reads `x` and defines it (globally, or in the scope `o` opens
//! and `c` closes) as `x + a_i`, or defines nothing; it predicts `x`
//! (rightly, wrongly, or where it defines nothing) at a locality.
//! `r` reads `x` and `y`. Every schedule gives the sequential outputs,
//! and with right predictions a reader runs on them, not after its
//! definers.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use phitex_defuse::{Ctx, Effect, Engine, Field, Locality, MayDefine, NodeId, Operation, Report, Stop, ValueId, Values};

/// How a `d` node runs and what it predicts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum D {
    /// Defines `x + a`, predicted rightly.
    Right,
    /// Defines `x + a`, predicted `x + a + 100`.
    Wrong,
    /// Defines nothing, predicted `7`.
    Unmade,
    /// Defines `x + a`, not predicted.
    Unpredicted,
}

struct Ops {
    kinds: Vec<D>,
    /// Definitions are local, in a scope around the `d` nodes.
    local: bool,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Ops {
    /// `x` after `d_0 .. d_i` (sequentially, from 0).
    fn x_after(&self, i: usize) -> i64 {
        (0..=i)
            .filter(|&k| self.kinds[k] != D::Unmade)
            .map(|k| i64::try_from(k).unwrap() + 1)
            .sum()
    }
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "d" => None,
            "y" => Some(vec![(v.field("y"), Locality::Global)]),
            _ => Some(Vec::new()),
        }
    }
    fn may_define(&self, _: &[ValueId], _: &Values) -> MayDefine {
        MayDefine::Any
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> Effect {
        match tag(v, a).as_str() {
            "o" => Effect::Open,
            "c" => Effect::Close,
            _ => Effect::None,
        }
    }
    fn predicted_defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        if tag(v, a) != "d" {
            return None;
        }
        let i = usize::try_from(v.as_int(a[1]).unwrap()).unwrap();
        let x = self.x_after(i);
        match self.kinds[i] {
            D::Right => Some(vec![(v.field("x"), v.int(x))]),
            D::Wrong => Some(vec![(v.field("x"), v.int(x + 100))]),
            D::Unmade => Some(vec![(v.field("x"), v.int(7))]),
            D::Unpredicted => None,
        }
    }
    fn predicted_localities(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        (self.local && tag(v, a) == "d").then(|| vec![(v.field("x"), Locality::Local)])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let args = cx.args();
        let (fx, fy) = (v.field("x"), v.field("y"));
        match tag(v, &args).as_str() {
            "d" => {
                let i = usize::try_from(v.as_int(args[1]).unwrap()).unwrap();
                let prev = cx.use_field(fx)?.and_then(|p| v.as_int(p)).unwrap_or(0);
                if self.kinds[i] != D::Unmade {
                    let loc = if self.local { Locality::Local } else { Locality::Global };
                    cx.define_with(fx, v.int(prev + i64::try_from(i).unwrap() + 1), loc);
                }
                Ok(ValueId::UNIT)
            }
            "y" => {
                cx.define_with(fy, v.int(5), Locality::Global);
                Ok(ValueId::UNIT)
            }
            "r" => {
                let xv = cx.use_field(fx)?.and_then(|p| v.as_int(p)).unwrap_or(-1);
                let yv = cx.use_field(fy)?.and_then(|p| v.as_int(p)).unwrap_or(-1);
                Ok(v.int(xv * 1000 + yv))
            }
            _ => Ok(ValueId::UNIT),
        }
    }
}

struct Threads(usize);

impl phitex_defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0
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
            for _ in 0..self.0 {
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

/// The unit: `y`, then (`o`) the `d` nodes (`c`), then readers after
/// each `d` and at the end.
fn build(kinds: &[D], local: bool) -> (Engine, Vec<NodeId>) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops {
        kinds: kinds.to_vec(),
        local,
    }));
    let u = eng.new_unit();
    let s = |eng: &Engine, t: &str| vec![eng.values().str(t)];
    let mut rs = Vec::new();
    let a = s(&eng, "y");
    eng.push(u, op, a);
    if local {
        let a = s(&eng, "o");
        eng.push(u, op, a);
    }
    for i in 0..kinds.len() {
        let a = vec![eng.values().str("d"), eng.values().int(i64::try_from(i).unwrap())];
        eng.push(u, op, a);
        let a = s(&eng, "r");
        rs.push(eng.push(u, op, a));
    }
    if local {
        let a = s(&eng, "c");
        eng.push(u, op, a);
    }
    let a = s(&eng, "r");
    rs.push(eng.push(u, op, a));
    (eng, rs)
}

fn outs(eng: &Engine, rs: &[NodeId]) -> Vec<Option<i64>> {
    rs.iter()
        .map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x)))
        .collect()
}

fn eval(eng: &mut Engine, sched: u64, threads: usize) -> Report {
    let r = if sched == 0 {
        eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::Predicted, 256),
            &mut Threads(threads),
        )
    } else {
        eng.evaluate(&mut phitex_defuse::RandomScheduler::new(sched, true), &mut Threads(threads))
    };
    r.expect("evaluate")
}

const MIXES: [&[D]; 4] = [
    &[D::Right; 12],
    &[
        D::Right,
        D::Wrong,
        D::Right,
        D::Unmade,
        D::Right,
        D::Right,
        D::Unpredicted,
        D::Right,
    ],
    &[D::Unmade, D::Unmade, D::Right, D::Wrong, D::Wrong, D::Right],
    &[
        D::Unpredicted,
        D::Right,
        D::Unmade,
        D::Right,
        D::Wrong,
        D::Right,
        D::Right,
    ],
];

#[test]
fn every_schedule_gives_the_sequential_outputs() {
    for local in [false, true] {
        for (m, kinds) in MIXES.iter().enumerate() {
            let (mut seq, srs) = build(kinds, local);
            seq.evaluate_sequential().expect("sequential");
            let want = outs(&seq, &srs);
            for sched in 0..24 {
                let (mut eng, rs) = build(kinds, local);
                eval(&mut eng, sched, 1 + usize::try_from(sched % 5).unwrap());
                assert_eq!(outs(&eng, &rs), want, "mix {m} local {local} schedule {sched}");
                assert!(
                    eng.check_reads().is_empty(),
                    "mix {m} local {local} schedule {sched}: {:?}",
                    eng.check_reads()
                );
            }
        }
    }
}

/// Right predictions: the readers read them (and stand), in parallel and
/// in steps alike.
#[test]
fn readers_take_right_predictions() {
    let (mut eng, rs) = build(MIXES[0], false);
    let rep = eval(&mut eng, 0, 4);
    let (mut seq, srs) = build(MIXES[0], false);
    seq.evaluate_sequential().expect("sequential");
    assert_eq!(outs(&eng, &rs), outs(&seq, &srs));
    assert!(rep.predictions_confirmed > 0, "{rep:?}");
    assert_eq!(rep.predictions_refuted, 0, "{rep:?}");
}
