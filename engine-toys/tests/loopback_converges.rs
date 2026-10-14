//! Toy: loop-back fields whose fixpoint is reached in 2 rounds.
//!
//! Program (one unit), 40 groups `j`, each of five nodes:
//! `["w", x]` contributes `x` to the sum accumulator `pos`;
//! `["r", (j + 5) % 40]` reads loop-back field `lb{(j+5)%40}` (a forward
//! read: its publisher comes later) and outputs it;
//! `["p", j]` publishes `lb{j}` = the fold of `pos` before it;
//! `["r", (j + 37) % 40]` (a backward read) and `["r", j]`.
//! Readers contribute 2 to `pos` whatever they read, so the publications
//! never depend on a loop-back value: from any start guess, round 2 reads
//! the true values and publishes them again.
//!
//! Expected of an ideal engine:
//! - cold, from any start guess (0, -1, a string, and the true values):
//!   at most 2 rounds (1 when the guess is the truth), outputs identical to
//!   the analytic values, runs = 200 nodes + the readers whose guess was
//!   wrong;
//! - an edit of one `w` (its contribution +1) under `LoopStart::Previous`:
//!   2 rounds; runs = the edited node, the publishers after it, and the
//!   readers of their fields; outputs equal a cold build of the new program;
//! - the same edit under `LoopStart::Initial`: at most 2 rounds, outputs
//!   identical.
//!
//! Run with `--nocapture` for the numbers.

#![allow(clippy::many_single_char_names)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use defuse::{Ctx, Engine, Field, Locality, LoopStart, NodeId, Operation, Report, Stop, Value, ValueId, Values};

const G: usize = 40;

/// Completed runs (a stop is not counted; see `Report::stops`): 0 `w`, 1 `p`, 2 `r`.
struct Toy {
    runs: [AtomicU64; 3],
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    a.first().map_or_else(String::new, |&x| match &*v.get(x) {
        Value::Str(s) => s.to_string(),
        _ => String::new(),
    })
}

fn lb(v: &Values, i: i64) -> Field {
    v.field(&format!("lb{i}"))
}

impl Operation for Toy {
    fn name(&self) -> &'static str {
        "toy"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let pos = v.field("pos");
        match tag(v, &a).as_str() {
            "w" => {
                self.runs[0].fetch_add(1, Relaxed);
                cx.contribute(pos, a[1]);
                Ok(ValueId::UNIT)
            }
            "p" => {
                let at = cx.acc_range(pos, None)?;
                self.runs[1].fetch_add(1, Relaxed);
                cx.publish(lb(v, v.as_int(a[1]).unwrap()), at);
                Ok(ValueId::UNIT)
            }
            _ => {
                let got = cx.use_field(lb(v, v.as_int(a[1]).unwrap()))?;
                self.runs[2].fetch_add(1, Relaxed);
                cx.contribute(pos, v.int(2));
                Ok(got.unwrap_or(ValueId::UNIT))
            }
        }
    }
}

struct Threads;

impl defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        4
    }
    fn execute(&mut self, n: usize, run: &(dyn Fn(usize) -> defuse::RunResult + Sync), out: &mut Vec<defuse::RunResult>) {
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
        _ => eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 256), &mut Threads),
    }
    .expect("evaluate")
}

/// The fields each group's three readers read.
fn reads(j: usize) -> [usize; 3] {
    [(j + 5) % G, (j + 37) % G, j]
}

/// The true publication of every field, for widths `ws`.
fn truth(ws: &[i64]) -> Vec<i64> {
    let mut at = 0;
    let mut out = vec![0; G];
    for j in 0..G {
        at += ws[j] + 2; // w, then the forward reader
        out[j] = at;
        at += 4; // the two readers after p
    }
    out
}

fn widths() -> Vec<i64> {
    (0..G).map(|j| 1 + i64::try_from(j % 4).unwrap()).collect()
}

#[derive(Clone, Copy, Debug)]
enum Guess {
    Zero,
    MinusOne,
    Str,
    Truth,
}

struct Built {
    eng: Engine,
    toy: Arc<Toy>,
    op: defuse::OpId,
    ws: Vec<NodeId>,
    rs: Vec<(usize, NodeId)>,
}

fn build(guess: Guess, start: Option<LoopStart>) -> Built {
    let mut eng = Engine::new();
    let toy = Arc::new(Toy { runs: Default::default() });
    let op = eng.register(toy.clone());
    let pos = eng.values().field("pos");
    eng.declare_accumulator(pos, Arc::new(defuse::SumMonoid));
    let t = truth(&widths());
    for i in 0..G {
        let v = eng.values();
        let init = match guess {
            Guess::Zero => v.int(0),
            Guess::MinusOne => v.int(-1),
            Guess::Str => v.str("??"),
            Guess::Truth => v.int(t[i]),
        };
        let f = lb(v, i64::try_from(i).unwrap());
        eng.declare_loopback(f, init);
    }
    if let Some(s) = start {
        eng.set_loop_start(s);
    }
    let u = eng.new_unit();
    let mut ws = Vec::new();
    let mut rs = Vec::new();
    let ws_v = widths();
    for j in 0..G {
        let jj = |x: usize| i64::try_from(x).unwrap();
        let [f, b, s] = reads(j);
        let a = vec![eng.values().str("w"), eng.values().int(ws_v[j])];
        ws.push(eng.push(u, op, a));
        let a = vec![eng.values().str("r"), eng.values().int(jj(f))];
        rs.push((f, eng.push(u, op, a)));
        let a = vec![eng.values().str("p"), eng.values().int(jj(j))];
        eng.push(u, op, a);
        let a = vec![eng.values().str("r"), eng.values().int(jj(b))];
        rs.push((b, eng.push(u, op, a)));
        let a = vec![eng.values().str("r"), eng.values().int(jj(s))];
        rs.push((s, eng.push(u, op, a)));
    }
    Built { eng, toy, op, ws, rs }
}

fn take(t: &Toy) -> [u64; 3] {
    [t.runs[0].swap(0, Relaxed), t.runs[1].swap(0, Relaxed), t.runs[2].swap(0, Relaxed)]
}

fn outs(b: &Built) -> Vec<Option<i64>> {
    b.rs.iter().map(|&(_, n)| b.eng.output(n).and_then(|x| b.eng.values().as_int(x))).collect()
}

fn want_outs(b: &Built, t: &[i64]) -> Vec<Option<i64>> {
    b.rs.iter().map(|&(f, _)| Some(t[f])).collect()
}

fn line(r: &Report) -> String {
    format!(
        "runs={} nodes_run={} stops={} rounds={} restarts={} rcp={}",
        r.runs, r.nodes_run, r.stops, r.rounds, r.restarts, r.realised_critical_path
    )
}

#[test]
fn loopback_converges() {
    let mut fails: BTreeMap<String, u32> = BTreeMap::new();
    let mut fail = |s: String| *fails.entry(s).or_insert(0) += 1;
    let t0 = truth(&widths());
    for sched in SCHEDS {
        for guess in [Guess::Zero, Guess::MinusOne, Guess::Str, Guess::Truth] {
            let name = format!("{sched} guess={guess:?}");
            let mut b = build(guess, None);
            let r = eval(&mut b.eng, sched);
            let k = take(&b.toy);
            println!("{name} cold: client w/p/r={k:?} {}", line(&r));
            if outs(&b) != want_outs(&b, &t0) {
                fail(format!("{name} cold: outputs differ from the fixpoint"));
            }
            let want_rounds = if matches!(guess, Guess::Truth) { 1 } else { 2 };
            if r.rounds > want_rounds {
                fail(format!("{name} cold: {} rounds > {want_rounds}", r.rounds));
            }
            let wrong_readers = if matches!(guess, Guess::Truth) { 0 } else { 3 * G as u64 };
            if k[0] + k[1] + k[2] > 5 * G as u64 + wrong_readers {
                fail(format!(
                    "{name} cold: {} client runs > {}",
                    k[0] + k[1] + k[2],
                    5 * G as u64 + wrong_readers
                ));
            }
        }
        for start in [LoopStart::Previous, LoopStart::Initial] {
            let name = format!("{sched} start={start:?}");
            let mut b = build(Guess::Zero, Some(start));
            eval(&mut b.eng, sched);
            take(&b.toy);
            // Group 20's contribution +1: lb20..lb39 move by 1.
            let e = 20;
            let mut ws = widths();
            ws[e] += 1;
            let a = vec![b.eng.values().str("w"), b.eng.values().int(ws[e])];
            b.eng.replace(b.ws[e], b.op, a);
            let r = eval(&mut b.eng, sched);
            let k = take(&b.toy);
            let t1 = truth(&ws);
            let moved = (0..G).filter(|&i| t0[i] != t1[i]).count();
            let readers_of_moved = b.rs.iter().filter(|&&(f, _)| t0[f] != t1[f]).count();
            println!(
                "{name} edit: client w/p/r={k:?} (moved fields {moved}, their readers {readers_of_moved}) {}",
                line(&r)
            );
            if outs(&b) != want_outs(&b, &t1) {
                fail(format!("{name} edit: outputs differ from the new fixpoint"));
            }
            if r.rounds > 2 {
                fail(format!("{name} edit: {} rounds > 2", r.rounds));
            }
            if matches!(start, LoopStart::Previous) {
                let want = [1, moved as u64, readers_of_moved as u64];
                if k != want {
                    fail(format!("{name} edit: client w/p/r runs {k:?}, want {want:?}"));
                }
            }
        }
    }
    for (f, n) in &fails {
        println!("FAIL x{n} {f}");
    }
    assert!(fails.is_empty(), "{} failures (see FAIL lines)", fails.len());
}
