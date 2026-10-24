//! `Operation::final_only`: a final node is held while loop-back fields
//! move and runs once the rounds converge. `inc k` reads loop-back `n`
//! (initially 0) and publishes `min(n + 1, k)`, so the rounds climb to `k`;
//! the final writer `w` reads `n` and defines `out`; a reader `r` after it
//! reads `out`. The writer runs exactly once per evaluation and sees the
//! converged value; edits rerun it only in the final phase; without rounds
//! it still runs once; a final node that moves a loop-back (`m`) fails.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use phitex_defuse::{Ctx, Engine, EvalError, Field, Locality, NodeId, OpId, Operation, Report, Stop, ValueId, Values};

struct Ops {
    writer_runs: AtomicU64,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "w" | "bad" => vec![(v.field("out"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn final_only(&self, a: &[ValueId], v: &Values) -> bool {
        matches!(tag(v, a).as_str(), "w" | "bad")
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let n = |cx: &mut Ctx<'_>| -> Result<i64, Stop> {
            let v = cx.values();
            Ok(cx.use_field(v.field("n"))?.and_then(|x| v.as_int(x)).unwrap_or(-1))
        };
        match tag(v, &a).as_str() {
            "inc" => {
                let k = v.as_int(a[1]).unwrap();
                let x = n(cx)?;
                cx.publish(v.field("n"), v.int((x + 1).min(k)));
                Ok(v.int(x))
            }
            "w" | "bad" => {
                self.writer_runs.fetch_add(1, Relaxed);
                let x = n(cx)?;
                // Tagged with its own argument, so an edit of the writer
                // shows in its output.
                let out = v.int(x * 100 + v.as_int(a[1]).unwrap_or(0));
                cx.define(v.field("out"), out);
                if tag(v, &a) == "bad" {
                    // Moves loop-back `m` (nobody else publishes it) after
                    // convergence.
                    cx.publish(v.field("m"), v.int(x + 10));
                }
                Ok(out)
            }
            "rp" => {
                // Not final, but reads the writer: it runs in the final
                // phase, and its publication moves `m` there.
                let out = cx.use_field(v.field("out"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                cx.publish(v.field("m"), v.int(out));
                Ok(v.int(out))
            }
            _ => Ok(cx.use_field(v.field("out"))?.unwrap_or(ValueId::UNIT)),
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
        out.extend(slots.into_iter().map(|s| s.into_inner().expect("ran")));
    }
}

fn eval(eng: &mut Engine, i: usize) -> Result<Report, EvalError> {
    match i % 3 {
        0 => eng.evaluate_sequential(),
        1 => eng.evaluate(&mut phitex_defuse::Greedy::default(), &mut Threads(4)),
        _ => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 64),
            &mut Threads(3),
        ),
    }
}

struct Built {
    eng: Engine,
    ops: Arc<Ops>,
    op: OpId,
    inc: NodeId,
    w: NodeId,
    r: NodeId,
}

fn build(loops: bool, k: i64) -> Built {
    let ops = Arc::new(Ops {
        writer_runs: AtomicU64::new(0),
    });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    let u = eng.new_unit();
    if loops {
        let (n, m, zero) = (eng.values().field("n"), eng.values().field("m"), eng.values().int(0));
        eng.declare_loopback(n, zero);
        eng.declare_loopback(m, zero);
    }
    let a = vec![eng.values().str("inc"), eng.values().int(k)];
    let inc = eng.push(u, op, a);
    let a = vec![eng.values().str("w"), eng.values().int(1)];
    let w = eng.push(u, op, a);
    let a = vec![eng.values().str("r")];
    let r = eng.push(u, op, a);
    Built {
        eng,
        ops,
        op,
        inc,
        w,
        r,
    }
}

fn int(eng: &Engine, n: NodeId) -> Option<i64> {
    eng.output(n).and_then(|x| eng.values().as_int(x))
}

#[test]
fn a_final_node_runs_once_on_the_converged_value() {
    for i in 0..30 {
        let mut b = build(true, 3);
        let rep = eval(&mut b.eng, i).expect("evaluate");
        assert!(rep.rounds >= 3, "{i}: {} rounds", rep.rounds);
        assert_eq!(b.ops.writer_runs.load(Relaxed), 1, "{i}: the writer ran more than once");
        assert_eq!(rep.final_nodes_run, 1, "{i}");
        assert_eq!(int(&b.eng, b.w), Some(301), "{i}: writer");
        assert_eq!(int(&b.eng, b.r), Some(301), "{i}: reader after it");
        assert!(b.eng.check_reads().is_empty(), "{i}: {:?}", b.eng.check_reads());
        // The loop moves on: the writer runs once more, at the end.
        let a = vec![b.eng.values().str("inc"), b.eng.values().int(5)];
        b.eng.replace(b.inc, b.op, a);
        let rep = eval(&mut b.eng, i + 1).expect("edit inc");
        assert_eq!(b.ops.writer_runs.load(Relaxed), 2, "{i}: edit inc");
        assert_eq!(rep.final_nodes_run, 1, "{i}: edit inc");
        assert_eq!(int(&b.eng, b.r), Some(501), "{i}: edit inc");
        // The writer itself edited: it runs in the final phase only.
        let a = vec![b.eng.values().str("w"), b.eng.values().int(7)];
        b.eng.replace(b.w, b.op, a);
        let rep = eval(&mut b.eng, i + 2).expect("edit w");
        assert_eq!(b.ops.writer_runs.load(Relaxed), 3, "{i}: edit w");
        assert_eq!(rep.final_nodes_run, 1, "{i}: edit w");
        assert_eq!(int(&b.eng, b.r), Some(507), "{i}: edit w");
        assert!(b.eng.check_reads().is_empty(), "{i}: {:?}", b.eng.check_reads());
    }
}

#[test]
fn without_rounds_a_final_node_still_runs_once() {
    for i in 0..9 {
        let mut b = build(false, 3);
        let rep = eval(&mut b.eng, i).expect("evaluate");
        assert_eq!(rep.rounds, 1, "{i}");
        assert_eq!(b.ops.writer_runs.load(Relaxed), 1, "{i}");
        assert_eq!(rep.final_nodes_run, 1, "{i}");
        // `n` is no loop-back here: the writer reads it unset.
        assert_eq!(int(&b.eng, b.r), Some(-99), "{i}");
    }
}

#[test]
fn a_final_node_moving_a_loop_back_fails() {
    for i in 0..9 {
        let mut b = build(true, 3);
        let a = vec![b.eng.values().str("bad"), b.eng.values().int(1)];
        b.eng.replace(b.w, b.op, a);
        match eval(&mut b.eng, i) {
            Err(EvalError::FinalMovedLoop { node, field }) => {
                assert_eq!(node, b.w, "{i}");
                assert_eq!(field, b.eng.values().field("m"), "{i}");
            }
            r => panic!("{i}: expected FinalMovedLoop, got {r:?}"),
        }
    }
}

#[test]
fn a_reader_of_a_final_node_moving_a_loop_back_fails_naming_it() {
    for i in 0..9 {
        let mut b = build(true, 3);
        let a = vec![b.eng.values().str("rp")];
        b.eng.replace(b.r, b.op, a);
        match eval(&mut b.eng, i) {
            Err(EvalError::FinalMovedLoop { node, field }) => {
                assert_eq!(node, b.r, "{i}");
                assert_eq!(field, b.eng.values().field("m"), "{i}");
            }
            r => panic!("{i}: expected FinalMovedLoop, got {r:?}"),
        }
    }
}

#[test]
fn at_the_round_cap_finalize_runs_the_final_phase_on_the_last_round() {
    for i in 0..9 {
        // `inc 1000` never converges within the cap.
        let mut b = build(true, 1000);
        b.eng.set_max_rounds(5);
        b.eng.set_round_cap_policy(phitex_defuse::CapPolicy::Finalize);
        let rep = eval(&mut b.eng, i).expect("finalized at the cap");
        assert_eq!(rep.rounds, 5, "{i}");
        assert_eq!(rep.capped, Some(vec![b.eng.values().field("n")]), "{i}");
        assert_eq!(b.ops.writer_runs.load(Relaxed), 1, "{i}");
        assert_eq!(rep.final_nodes_run, 1, "{i}");
        // Round 5 ran with n = 4.
        assert_eq!(int(&b.eng, b.w), Some(401), "{i}: writer");
        assert_eq!(int(&b.eng, b.r), Some(401), "{i}: reader");
        // The default policy fails there.
        let mut b = build(true, 1000);
        b.eng.set_max_rounds(5);
        assert!(
            matches!(eval(&mut b.eng, i), Err(EvalError::NonConvergence { rounds: 5 })),
            "{i}"
        );
        assert_eq!(b.ops.writer_runs.load(Relaxed), 0, "{i}: no final phase after an error");
    }
}

/// From the previous evaluation's loop-back values (`LoopStart::Previous`,
/// PhiTeX's): an edit that takes rounds again dirties the final node, which
/// waits for the final phase instead of running in every round.
#[test]
fn from_previous_values_a_final_node_runs_once_per_evaluation() {
    for i in 0..9 {
        let mut b = build(true, 3);
        b.eng.set_loop_start(phitex_defuse::LoopStart::Previous);
        let rep = eval(&mut b.eng, i).expect("evaluate");
        assert!(rep.rounds >= 3, "{i}: {} rounds", rep.rounds);
        assert_eq!(b.ops.writer_runs.load(Relaxed), 1, "{i}: cold");
        let a = vec![b.eng.values().str("inc"), b.eng.values().int(5)];
        b.eng.replace(b.inc, b.op, a);
        let rep = eval(&mut b.eng, i + 1).expect("edit inc");
        assert!(rep.rounds >= 2, "{i}: {} rounds after the edit", rep.rounds);
        assert_eq!(b.ops.writer_runs.load(Relaxed), 2, "{i}: the writer ran once more");
        assert_eq!(int(&b.eng, b.r), Some(501), "{i}: edit inc");
    }
}

/// A one-round edit: the final node made dirty runs in the round (with
/// the other work, not after it), once, and no final phase follows.
#[test]
fn a_one_round_edit_runs_its_final_node_in_the_round() {
    for i in 0..9 {
        let mut b = build(false, 3);
        b.eng.set_loop_start(phitex_defuse::LoopStart::Previous);
        eval(&mut b.eng, i).expect("evaluate");
        let before = b.ops.writer_runs.load(Relaxed);
        let a = vec![b.eng.values().str("w"), b.eng.values().int(7)];
        b.eng.replace(b.w, b.op, a);
        let rep = eval(&mut b.eng, i + 1).expect("edit w");
        assert_eq!(rep.rounds, 1, "{i}");
        assert_eq!(b.ops.writer_runs.load(Relaxed), before + 1, "{i}: once");
        assert_eq!(rep.final_nodes_run, 0, "{i}: no final phase");
        // (No loop-backs: `n` reads as -1.)
        assert_eq!(int(&b.eng, b.w), Some(-93), "{i}");
    }
}
