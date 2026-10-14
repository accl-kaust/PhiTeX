//! Toy: an edit re-makes one node whose contributions are identical, with
//! 200 range readers after it.
//!
//! Program (one unit): 10 contributors `["c", k, mark]` (each contributes
//! `k` to the sum accumulator `acc`), the edited node `E`, then 200 readers
//! `["r"]` (each outputs the fold of `acc` from the unit start up to
//! itself), with a contributor after every 10 readers.
//!
//! Edits keep `E`'s contribution and change only its text:
//! - `direct`: `E` is `["c", 5, mark]`; the edit changes `mark`.
//! - `parent`: `E` is `["p", 5, mark]`, which unfolds one child
//!   `["c", 5, "child"]`; the edit changes the parent's `mark`, so the
//!   parent runs again and makes its child again, with the same text.
//!
//! Each variant runs with and without `static_contribs` declared, under
//! four schedules. An ideal engine, per edit: the edited node runs once
//! (the parent variant: the parent once, its child at most once), no reader
//! runs, the realised critical path is 1 (2 for the parent variant), and
//! every output is unchanged. The storm: 20 edits evaluated one by one
//! (each as above), then 20 contributors re-texted in one evaluation (20
//! runs, no reader).
//!
//! Run with `--nocapture` for the numbers; failures are collected and
//! asserted at the end.

#![allow(clippy::too_many_lines, clippy::needless_range_loop)] // (Phi's toy, kept as written)
#![allow(clippy::many_single_char_names)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Report, Stop, Value, ValueId, Values};

const READERS: usize = 200;

/// Completed runs (a stop is not counted; see `Report::stops`) by kind: 0 contributors (`c`), 1 readers (`r`), 2 parents (`p`).
struct Toy {
    declare: bool,
    runs: [AtomicU64; 3],
    me: std::sync::OnceLock<OpId>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    a.first().map_or_else(String::new, |&x| match &*v.get(x) {
        Value::Str(s) => s.to_string(),
        _ => String::new(),
    })
}

impl Operation for Toy {
    fn name(&self) -> &'static str {
        "toy"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        if !self.declare {
            return None;
        }
        match tag(v, a).as_str() {
            "c" => Some(vec![(v.field("acc"), a[1])]),
            _ => Some(Vec::new()),
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let acc = v.field("acc");
        match tag(v, &a).as_str() {
            "c" => {
                self.runs[0].fetch_add(1, Relaxed);
                cx.contribute(acc, a[1]);
                Ok(ValueId::UNIT)
            }
            "p" => {
                self.runs[2].fetch_add(1, Relaxed);
                let op = *self.me.get().expect("op");
                cx.child(op, vec![v.str("c"), a[1], v.str("child")]);
                Ok(ValueId::UNIT)
            }
            _ => {
                let fold = cx.acc_range(acc, None)?;
                self.runs[1].fetch_add(1, Relaxed);
                Ok(fold)
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

const SCHEDS: [&str; 4] = ["seq", "spec", "random", "par"];

fn eval(eng: &mut Engine, sched: &str) -> Report {
    match sched {
        "seq" => eng.evaluate_sequential(),
        "spec" => eng.evaluate(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
            &mut defuse::Sequential,
        ),
        "random" => eng.evaluate(&mut defuse::RandomScheduler::new(7, true), &mut defuse::Sequential),
        _ => eng.evaluate(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
            &mut Threads,
        ),
    }
    .expect("evaluate")
}

struct Built {
    eng: Engine,
    toy: Arc<Toy>,
    op: OpId,
    edited: NodeId,
    contributors: Vec<NodeId>,
    readers: Vec<NodeId>,
}

fn build(declare: bool, parent: bool) -> Built {
    let mut eng = Engine::new();
    let toy = Arc::new(Toy {
        declare,
        runs: Default::default(),
        me: std::sync::OnceLock::new(),
    });
    let op = eng.register(toy.clone());
    let _ = toy.me.set(op);
    let acc = eng.values().field("acc");
    eng.declare_accumulator(acc, Arc::new(defuse::SumMonoid));
    let u = eng.new_unit();
    let c = |eng: &Engine, k: i64, mark: &str| {
        let v = eng.values();
        vec![v.str("c"), v.int(k), v.str(mark)]
    };
    let mut contributors = Vec::new();
    for k in 1..=10 {
        let a = c(&eng, k, "m");
        contributors.push(eng.push(u, op, a));
    }
    let a = if parent {
        let v = eng.values();
        vec![v.str("p"), v.int(5), v.str("e0")]
    } else {
        c(&eng, 5, "e0")
    };
    let edited = eng.push(u, op, a);
    let mut readers = Vec::new();
    for i in 0..READERS {
        let a = vec![eng.values().str("r")];
        readers.push(eng.push(u, op, a));
        if i % 10 == 9 {
            let a = c(&eng, 100 + i64::try_from(i).unwrap(), "m");
            contributors.push(eng.push(u, op, a));
        }
    }
    Built {
        eng,
        toy,
        op,
        edited,
        contributors,
        readers,
    }
}

fn outputs(eng: &Engine, ids: &[NodeId]) -> Vec<Option<i64>> {
    ids.iter()
        .map(|&i| eng.output(i).and_then(|x| eng.values().as_int(x)))
        .collect()
}

fn take(t: &Toy) -> [u64; 3] {
    [
        t.runs[0].swap(0, Relaxed),
        t.runs[1].swap(0, Relaxed),
        t.runs[2].swap(0, Relaxed),
    ]
}

fn line(r: &Report) -> String {
    format!(
        "runs={} nodes_run={} stops={} rounds={} rcp={} cp={} rerun_causes={} static_refuted={}",
        r.runs,
        r.nodes_run,
        r.stops,
        r.rounds,
        r.realised_critical_path,
        r.critical_path,
        r.rerun_causes.len(),
        r.static_contribs_refuted
    )
}

#[test]
fn edit_storm_remade_node() {
    let mut fails: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    let mut fail = |s: String| *fails.entry(s).or_insert(0) += 1;
    for parent in [false, true] {
        for declare in [false, true] {
            for sched in SCHEDS {
                let name = format!("{} declare={declare} {sched}", if parent { "parent" } else { "direct" });
                let mut b = build(declare, parent);
                let r = eval(&mut b.eng, sched);
                let k = take(&b.toy);
                println!("{name} cold: client c/r/p={k:?} {}", line(&r));
                let reference = outputs(&b.eng, &b.readers);
                // The storm, one evaluation per edit.
                let mut worst = (0u64, 0u64, 0u32, 0u64);
                for n in 1..=20 {
                    let mark = format!("e{n}");
                    let a = {
                        let v = b.eng.values();
                        vec![v.str(if parent { "p" } else { "c" }), v.int(5), v.str(&mark)]
                    };
                    b.eng.replace(b.edited, b.op, a);
                    let r = eval(&mut b.eng, sched);
                    let k = take(&b.toy);
                    if n == 1 {
                        println!("{name} edit 1: client c/r/p={k:?} {}", line(&r));
                        if !r.rerun_causes.is_empty() {
                            println!(
                                "{name} edit 1 causes: {:?}",
                                &r.rerun_causes[..r.rerun_causes.len().min(6)]
                            );
                        }
                    }
                    worst = (
                        worst.0.max(k[1]),
                        worst.1.max(r.runs),
                        worst.2.max(r.realised_critical_path),
                        worst.3.max(k[0] + k[2]),
                    );
                    let out = outputs(&b.eng, &b.readers);
                    if out != reference {
                        fail(format!("{name} edit: outputs changed"));
                    }
                    let (want_runs, want_rcp) = if parent { (2, 2) } else { (1, 1) };
                    if k[1] != 0 {
                        fail(format!("{name} edit: {} readers ran, want 0", k[1]));
                    }
                    if parent {
                        if k[2] != 1 || k[0] > 1 {
                            fail(format!("{name} edit: parent/child runs {}/{}, want 1/<=1", k[2], k[0]));
                        }
                    } else if k[0] != 1 {
                        fail(format!("{name} edit: edited node ran {} times, want 1", k[0]));
                    }
                    if r.runs > want_runs {
                        fail(format!("{name} edit: report runs {} > {want_runs}", r.runs));
                    }
                    if r.realised_critical_path > want_rcp {
                        fail(format!(
                            "{name} edit: realised critical path {} > {want_rcp}",
                            r.realised_critical_path
                        ));
                    }
                }
                println!(
                    "{name} storm of 20: worst readers run={} report runs={} rcp={} edited runs={}",
                    worst.0, worst.1, worst.2, worst.3
                );
                // 20 contributors re-texted in one evaluation, same contributions.
                let cs = b.contributors.clone();
                for (i, &c) in cs.iter().take(20).enumerate() {
                    let old = b
                        .eng
                        .values()
                        .as_int(b.eng.node_info(c).map_or(ValueId::UNIT, |ni| ni.args[1]));
                    let a = {
                        let v = b.eng.values();
                        vec![v.str("c"), v.int(old.unwrap_or(0)), v.str(&format!("b{i}"))]
                    };
                    b.eng.replace(c, b.op, a);
                }
                let r = eval(&mut b.eng, sched);
                let k = take(&b.toy);
                println!("{name} batch of 20: client c/r/p={k:?} {}", line(&r));
                if outputs(&b.eng, &b.readers) != reference {
                    fail(format!("{name} batch: outputs changed"));
                }
                if k[1] != 0 || k[0] != 20 {
                    fail(format!(
                        "{name} batch: contributor/reader runs {}/{}, want 20/0",
                        k[0], k[1]
                    ));
                }
                if r.realised_critical_path > 1 {
                    fail(format!(
                        "{name} batch: realised critical path {} > 1",
                        r.realised_critical_path
                    ));
                }
            }
        }
    }
    for (f, n) in &fails {
        println!("FAIL x{n} {f}");
    }
    assert!(fails.is_empty(), "{} failures (see FAIL lines)", fails.len());
}
