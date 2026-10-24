//! Progress under bad predictions: a node whose run was refuted twice runs
//! on committed values only for the rest of the evaluation, so each node
//! is refuted a few times at most and the evaluation ends (in the worst
//! case in text order). Finishers `F_i` each read the previous finisher's `d` and
//! predict `d = 100`, which never holds: every speculative read of `d` is
//! refuted.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Report, Stop, ValueId, Values};

struct Ops;

impl Operation for Ops {
    fn defs(&self, _: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(vec![(v.field("d"), Locality::Local)])
    }
    fn predicted_defs(&self, _: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        Some(vec![(v.field("d"), v.int(100))])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let prev = cx.use_field(v.field("d"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
        let d = v.int(prev + v.as_int(a[0]).unwrap_or(0) + 1);
        cx.define(v.field("d"), d);
        Ok(d)
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

const N: usize = 40;

fn build(k: i64) -> (Engine, Vec<phitex_defuse::NodeId>, phitex_defuse::OpId) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let ns = (0..N)
        .map(|i| {
            let x = eng.values().int(i as i64 * k);
            eng.push(u, op, vec![x])
        })
        .collect();
    (eng, ns, op)
}

fn outs(eng: &Engine, ns: &[phitex_defuse::NodeId]) -> Vec<Option<i64>> {
    ns.iter()
        .map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x)))
        .collect()
}

fn eval(eng: &mut Engine, i: usize) -> Report {
    let spec = match i % 2 {
        0 => phitex_defuse::Speculation::Predicted,
        _ => phitex_defuse::Speculation::All,
    };
    eng.evaluate(&mut phitex_defuse::Greedy::speculative(spec, 256), &mut Threads(1 + i % 6))
        .expect("evaluate ends")
}

/// Each node is refuted at most three times an evaluation: it has two runs
/// on guesses that are refuted at most, and a run begun on guesses before
/// its second refutation, then runs on committed values (a rerun per
/// change of what it read, stops that resume), so the runs stay linear in
/// the nodes: under 5 per node in every schedule here, under load too.
fn at_most_once(rep: &Report, what: &str) {
    assert!(rep.nodes_conservative <= N as u64, "{what}: {}", rep.nodes_conservative);
    assert!(
        rep.refutations_max <= 3,
        "{what}: a node refuted {} times",
        rep.refutations_max
    );
    assert!(
        rep.runs <= 5 * N as u64,
        "{what}: {} runs for {N} nodes ({} gone conservative)",
        rep.runs,
        rep.nodes_conservative
    );
}

#[test]
fn refuted_predictions_end_in_order() {
    for i in 0..60 {
        let (mut eng, ns, op) = build(1);
        let rep = eval(&mut eng, i);
        at_most_once(&rep, &format!("cold {i}"));
        let (mut seq, sns, _) = build(1);
        seq.evaluate_sequential().expect("sequential");
        assert_eq!(outs(&eng, &ns), outs(&seq, &sns), "cold {i}");
        assert!(eng.check_reads().is_empty(), "cold {i}: {:?}", eng.check_reads());
        // An edit at the front changes every finisher after it.
        let x = eng.values().int(7);
        eng.replace(ns[0], op, vec![x]);
        let rep = eval(&mut eng, i + 1);
        at_most_once(&rep, &format!("edit {i}"));
        assert!(eng.check_reads().is_empty(), "edit {i}: {:?}", eng.check_reads());
    }
}
