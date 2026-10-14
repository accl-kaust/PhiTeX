//! Toy: a contribution change at node k re-runs exactly the range readers
//! after k whose fold changed.
//!
//! Program (one unit): 100 contributors `["c", i, x]` (each contributes `x`
//! to the max accumulator `acc`), each followed by a reader `["r", i, w]`
//! that outputs the fold of `acc` over the window after contributor
//! `i - w` (exclusive) up to itself: the last `w` contributors. Every fifth
//! reader instead folds from the unit start (`w = 0`). Window widths vary
//! from 1 to 8, so readers after k are of three kinds: windows that do not
//! hold k, windows that hold k whose max does not move, and windows whose
//! max moves.
//!
//! Edits re-text contributor k with a new value, one evaluation each:
//! raise to a new maximum, lower below every window maximum, change within
//! the window maxima (no fold changes), and lower a window maximum. The
//! expected set of readers to run is computed by folding every window
//! before and after. An ideal engine runs k once, exactly the readers whose
//! fold changed, nothing else; the realised critical path is 2 when a
//! reader runs, else 1; every output equals a fresh fold. Run with and
//! without `static_contribs` declared, under four schedules.
//!
//! Run with `--nocapture` for the numbers.

#![allow(clippy::too_many_lines, clippy::needless_range_loop)] // (Phi's toy, kept as written)
#![allow(clippy::many_single_char_names)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, RwLock};

use defuse::{Ctx, Engine, Field, Locality, Monoid, NodeId, OpId, Operation, Report, Stop, Value, ValueId, Values};

const N: usize = 100;

/// Integer max, identity -1 (every contribution is >= 0).
struct Max;

impl Monoid for Max {
    fn empty(&self, v: &Values) -> ValueId {
        v.int(-1)
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        v.int(v.as_int(a).unwrap_or(-1).max(v.as_int(b).unwrap_or(-1)))
    }
}

/// Completed runs (a stop is not counted; see `Report::stops`).
/// `runs[i]`: runs of contributor i; `runs[N + i]`: runs of reader i.
/// `ids[i]`: contributor i's node, the readers' window anchors.
struct Toy {
    declare: bool,
    runs: Vec<AtomicU64>,
    ids: RwLock<Vec<NodeId>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    a.first().map_or_else(String::new, |&x| match &*v.get(x) {
        Value::Str(s) => s.to_string(),
        _ => String::new(),
    })
}

fn int(v: &Values, x: ValueId) -> usize {
    usize::try_from(v.as_int(x).unwrap()).unwrap()
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
            "c" => Some(vec![(v.field("acc"), a[2])]),
            _ => Some(Vec::new()),
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let acc = v.field("acc");
        let i = int(v, a[1]);
        if tag(v, &a) == "c" {
            self.runs[i].fetch_add(1, Relaxed);
            cx.contribute(acc, a[2]);
            return Ok(ValueId::UNIT);
        }
        let w = int(v, a[2]);
        let from = (w > 0 && i >= w).then(|| self.ids.read().unwrap()[i - w]);
        let fold = cx.acc_range(acc, from)?;
        self.runs[N + i].fetch_add(1, Relaxed);
        Ok(fold)
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

/// Reader i's window width (0: from the unit start).
fn width(i: usize) -> usize {
    if i % 5 == 4 { 0 } else { 1 + (i * 7) % 8 }
}

/// The folds every reader should output, for contributions `xs`.
fn folds(xs: &[i64]) -> Vec<i64> {
    (0..N)
        .map(|i| {
            let w = width(i);
            let lo = if w > 0 && i >= w { i - w + 1 } else { 0 };
            xs[lo..=i].iter().copied().max().unwrap_or(-1)
        })
        .collect()
}

fn initial() -> Vec<i64> {
    let mut s: u64 = 12345;
    (0..N)
        .map(|_| {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            i64::try_from((s >> 33) % 50).unwrap()
        })
        .collect()
}

fn line(r: &Report) -> String {
    format!(
        "runs={} nodes_run={} stops={} rounds={} rcp={} static_refuted={}",
        r.runs, r.nodes_run, r.stops, r.rounds, r.realised_critical_path, r.static_contribs_refuted
    )
}

#[test]
fn range_readers_mixed() {
    let mut fails: BTreeMap<String, u32> = BTreeMap::new();
    let mut fail = |s: String| *fails.entry(s).or_insert(0) += 1;
    for declare in [false, true] {
        for sched in SCHEDS {
            let name = format!("declare={declare} {sched}");
            let mut eng = Engine::new();
            let toy = Arc::new(Toy {
                declare,
                runs: (0..2 * N).map(|_| AtomicU64::new(0)).collect(),
                ids: RwLock::new(Vec::new()),
            });
            let op: OpId = eng.register(toy.clone());
            let acc = eng.values().field("acc");
            eng.declare_accumulator(acc, Arc::new(Max));
            let u = eng.new_unit();
            let mut xs = initial();
            let mut cs = Vec::new();
            let mut rs = Vec::new();
            for i in 0..N {
                let ii = i64::try_from(i).unwrap();
                let a = {
                    let v = eng.values();
                    vec![v.str("c"), v.int(ii), v.int(xs[i])]
                };
                cs.push(eng.push(u, op, a));
                let a = {
                    let v = eng.values();
                    vec![v.str("r"), v.int(ii), v.int(i64::try_from(width(i)).unwrap())]
                };
                rs.push(eng.push(u, op, a));
            }
            toy.ids.write().unwrap().clone_from(&cs);
            let r = eval(&mut eng, sched);
            let total: u64 = toy.runs.iter().map(|x| x.swap(0, Relaxed)).sum();
            println!("{name} cold: client runs={total} {}", line(&r));
            let outs = |eng: &Engine| -> Vec<i64> {
                rs.iter()
                    .map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x)).unwrap_or(i64::MIN))
                    .collect()
            };
            if outs(&eng) != folds(&xs) {
                fail(format!("{name} cold: outputs differ from the folds"));
            }
            // (k, kind): the new value is picked from the current windows.
            for (k, kind) in [
                (10, "raise"),
                (50, "below"),
                (50, "within"),
                (40, "middle"),
                (90, "raise"),
                (30, "lower-max"),
                (70, "below"),
                (60, "middle"),
            ] {
                let before = folds(&xs);
                let max_after_k = (k..N).map(|i| before[i]).max().unwrap();
                let new = match kind {
                    "raise" => max_after_k + 1,
                    "below" => 0,
                    // Not above any window max holding k, not equal to old.
                    "within" => {
                        let cap = (k..N).filter(|&i| window_holds(i, k)).map(|i| before[i]).min().unwrap();
                        if xs[k] == cap { (cap - 1).max(0) } else { cap }
                    }
                    // Above some window maxima holding k, below others.
                    "middle" => {
                        let mut ms: Vec<i64> = (k..N).filter(|&i| window_holds(i, k)).map(|i| before[i]).collect();
                        ms.sort_unstable();
                        ms.dedup();
                        ms.get(ms.len() / 2).map_or(xs[k] + 1, |&m| m + 1)
                    }
                    // Lowered after first being made a window max (below).
                    _ => xs[k],
                };
                let new = if kind == "lower-max" {
                    // First make k the max (one evaluation, not counted), then lower it.
                    let a = mk(&eng, k, max_after_k + 1);
                    eng.replace(cs[k], op, a);
                    xs[k] = max_after_k + 1;
                    eval(&mut eng, sched);
                    for x in &toy.runs {
                        x.store(0, Relaxed);
                    }
                    0
                } else {
                    new
                };
                let before = folds(&xs);
                xs[k] = new;
                let after = folds(&xs);
                let want: Vec<usize> = (0..N).filter(|&i| before[i] != after[i]).collect();
                let a = mk(&eng, k, new);
                eng.replace(cs[k], op, a);
                let r = eval(&mut eng, sched);
                let ran_c: Vec<usize> = (0..N).filter(|&i| toy.runs[i].load(Relaxed) > 0).collect();
                let ran_r: Vec<usize> = (0..N).filter(|&i| toy.runs[N + i].load(Relaxed) > 0).collect();
                let reader_runs: u64 = (0..N).map(|i| toy.runs[N + i].load(Relaxed)).sum();
                let holding = (k..N).filter(|&i| window_holds(i, k)).count();
                for x in &toy.runs {
                    x.store(0, Relaxed);
                }
                println!(
                    "{name} edit k={k} {kind}: readers holding k={holding} fold changed={} ran={} (reader runs {reader_runs}) {}",
                    want.len(),
                    ran_r.len(),
                    line(&r)
                );
                if outs(&eng) != after {
                    fail(format!("{name} edit {kind}: outputs differ from the folds"));
                }
                if ran_c != vec![k] {
                    fail(format!("{name} edit {kind}: contributors ran {ran_c:?}, want [{k}]"));
                }
                if ran_r != want {
                    let extra = ran_r.iter().filter(|i| !want.contains(i)).count();
                    let missing = want.iter().filter(|i| !ran_r.contains(i)).count();
                    fail(format!(
                        "{name} edit {kind}: readers ran {} (extra {extra}, missing {missing}), want {}",
                        ran_r.len(),
                        want.len()
                    ));
                }
                if reader_runs > want.len() as u64 {
                    fail(format!(
                        "{name} edit {kind}: {reader_runs} reader runs for {} readers",
                        want.len()
                    ));
                }
                let want_rcp = if want.is_empty() { 1 } else { 2 };
                if r.realised_critical_path > want_rcp {
                    fail(format!(
                        "{name} edit {kind}: realised critical path {} > {want_rcp}",
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

fn window_holds(i: usize, k: usize) -> bool {
    let w = width(i);
    let lo = if w > 0 && i >= w { i - w + 1 } else { 0 };
    lo <= k && k <= i
}

fn mk(eng: &Engine, k: usize, x: i64) -> Vec<ValueId> {
    let v = eng.values();
    vec![v.str("c"), v.int(i64::try_from(k).unwrap()), v.int(x)]
}
