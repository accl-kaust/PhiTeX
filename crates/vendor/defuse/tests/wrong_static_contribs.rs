//! A wrong static contribution list costs a re-run, never a wrong result.
//! Execs `e` declare that they contribute nothing (`Some(vec![])`), so a
//! range reader passes them while they are in work; their runs contribute
//! to `v` all the same, after some work. Readers `p` fold `v` from the
//! unit start. Every schedule must end where a sequential build does,
//! with every recorded read holding.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use defuse::{Ctx, Engine, Field, Locality, Monoid, NodeId, Operation, Stop, ValueId, Values};

struct Sum;

impl Monoid for Sum {
    fn empty(&self, v: &Values) -> ValueId {
        v.int(0)
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        v.int(v.as_int(a).unwrap_or(0) + v.as_int(b).unwrap_or(0))
    }
}

struct Ops {
    ids: std::sync::RwLock<Vec<NodeId>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        (tag(v, a) == "p").then(|| vec![v.field("v")])
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        (tag(v, a) == "e").then(Vec::new)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) == "e" {
            let mut x = 0u64;
            for i in 0..30_000u64 {
                x = x.wrapping_mul(31).wrapping_add(i);
            }
            cx.contribute(v.field("v"), v.int(1 + i64::from(x == 7)));
            return Ok(ValueId::UNIT);
        }
        // Windows: from the start, after the node 7 before, and that
        // window ending 2 before this node.
        let i = usize::try_from(v.as_int(a[1]).unwrap()).unwrap();
        let ids = self.ids.read().unwrap().clone();
        match i % 3 {
            0 => cx.acc_range(v.field("v"), None),
            1 => cx.acc_range(v.field("v"), Some(ids[i.saturating_sub(7)])),
            _ => cx.acc_range_to(v.field("v"), Some(ids[i.saturating_sub(7)]), Some(ids[i - 2])),
        }
    }
}

struct Threads(usize);

impl defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0
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
        out.extend(slots.into_iter().map(|s| s.into_inner().expect("ran")));
    }
}

fn build() -> (Engine, Vec<NodeId>) {
    let mut eng = Engine::new();
    eng.declare_accumulator(eng.values().field("v"), Arc::new(Sum));
    let ops = Arc::new(Ops {
        ids: std::sync::RwLock::new(Vec::new()),
    });
    let op = eng.register(ops.clone());
    let u = eng.new_unit();
    let mut ps = Vec::new();
    for i in 0..60 {
        let t = if i % 3 == 2 { "p" } else { "e" };
        let a = vec![eng.values().str(t), eng.values().int(i)];
        let n = eng.push(u, op, a);
        ops.ids.write().unwrap().push(n);
        if t == "p" {
            ps.push(n);
        }
    }
    (eng, ps)
}

fn outs(eng: &Engine, ps: &[NodeId]) -> Vec<Option<i64>> {
    ps.iter()
        .map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x)))
        .collect()
}

#[test]
fn a_wrong_empty_declaration_is_refuted_not_believed() {
    let (mut seq, sps) = build();
    seq.evaluate_sequential().expect("sequential");
    let want = outs(&seq, &sps);
    for i in 0..40 {
        let (mut eng, ps) = build();
        let r = match i % 3 {
            0 => eng.evaluate(&mut defuse::Greedy::default(), &mut Threads(4)),
            1 => eng.evaluate(
                &mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256),
                &mut Threads(8),
            ),
            _ => eng.evaluate(
                &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
                &mut Threads(3),
            ),
        };
        r.expect("evaluate");
        assert_eq!(outs(&eng, &ps), want, "schedule {i}");
        assert!(eng.check_reads().is_empty(), "schedule {i}: {:?}", eng.check_reads());
    }
}
