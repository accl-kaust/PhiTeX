//! Counter registers as accumulators (PhiTeX's register folds): each
//! piece reads counter `acc:count:k` as the fold of the writes before it
//! and writes it by an affine map that does not depend on what it read
//! (`add c`, `set c`, 32-bit wrap). The counters are declared by prefix;
//! with `Monoid::speculate` a reader guesses past writers still running
//! (as they last wrote, or not at all) instead of waiting for them, so a
//! cold build settles in two waves, not a chain through every writer.
//! Every schedule gives the sequential values, cold and after edits.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use defuse::{Ctx, Engine, Field, Locality, Monoid, NodeId, Operation, Stop, ValueId, Values};

const N: usize = 400;
const K: usize = 3;

/// Affine maps `x -> a x + c` (a in {0, 1}), as `[a, c]`, composed in
/// position order (the later applied last).
struct Affine {
    guess: bool,
}

fn map_of(v: &Values, x: ValueId) -> (i64, i64) {
    v.with_list(x, |e| (v.as_int(e[0]).unwrap(), v.as_int(e[1]).unwrap())).unwrap_or((1, 0))
}

fn wrap(x: i64) -> i64 {
    i64::from(x as i32)
}

impl Monoid for Affine {
    fn empty(&self, v: &Values) -> ValueId {
        v.list(vec![v.int(1), v.int(0)])
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        // b after a: x -> b.a (a.a x + a.c) + b.c
        let ((aa, ac), (ba, bc)) = (map_of(v, a), map_of(v, b));
        v.list(vec![v.int(aa * ba), v.int(wrap(ba * ac + bc))])
    }
    fn speculate(&self) -> bool {
        self.guess
    }
}

/// Piece `i`'s write of counter `k`: `Some((a, c))`, or none.
fn write(i: usize, k: usize, bump: i64) -> Option<(i64, i64)> {
    match (i * 7 + k * 3) % 11 {
        0 => Some((0, i64::try_from(i % 5).unwrap() + bump)),
        1..=6 => Some((1, i64::try_from(k + 1).unwrap() + bump)),
        _ => None,
    }
}

struct Ops {
    runs: AtomicU64,
    busy: u64,
}

fn counter(v: &Values, k: usize) -> Field {
    v.field(&format!("acc:count:{k}"))
}

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Relaxed);
        let v = cx.values();
        let a = cx.args();
        let (i, bump) = (usize::try_from(v.as_int(a[0]).unwrap()).unwrap(), v.as_int(a[1]).unwrap());
        // Reads every counter (the fold before it, applied to 0)...
        let mut seen = Vec::new();
        for k in 0..K {
            let (_, c) = map_of(v, cx.acc_range(counter(v, k), None)?);
            seen.push(v.int(c));
        }
        let mut x = 0u64;
        for j in 0..self.busy {
            x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(j));
        }
        // ...and writes some, by maps that do not depend on what it read.
        for k in 0..K {
            if let Some((ma, mc)) = write(i, k, bump) {
                cx.contribute(counter(v, k), v.list(vec![v.int(ma), v.int(mc)]));
            }
        }
        Ok(v.list(seen))
    }
}

/// The counters each piece sees, sequentially.
fn reference(bumped: Option<usize>) -> Vec<Vec<i64>> {
    let mut c = [0i64; K];
    let mut out = Vec::new();
    for i in 0..N {
        out.push(c.to_vec());
        for (k, ck) in c.iter_mut().enumerate() {
            if let Some((a, m)) = write(i, k, i64::from(Some(i) == bumped)) {
                *ck = wrap(a * *ck + m);
            }
        }
    }
    out
}

fn build(guess: bool) -> (Engine, Arc<Ops>, Vec<NodeId>, defuse::OpId) {
    let mut eng = Engine::new();
    eng.declare_accumulator_prefix("acc:count:", Arc::new(Affine { guess }));
    let ops = Arc::new(Ops {
        runs: AtomicU64::new(0),
        busy: 20_000,
    });
    let op = eng.register(ops.clone());
    let u = eng.new_unit();
    let ids = (0..N)
        .map(|i| {
            let a = {
                let v = eng.values();
                vec![v.int(i64::try_from(i).unwrap()), v.int(0)]
            };
            eng.push(u, op, a)
        })
        .collect();
    (eng, ops, ids, op)
}

fn seen(eng: &Engine, ids: &[NodeId]) -> Vec<Vec<i64>> {
    let v = eng.values();
    ids.iter()
        .map(|&n| v.with_list(eng.output(n).expect("output"), |xs| xs.iter().map(|&x| v.as_int(x).unwrap()).collect()).unwrap())
        .collect()
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
            for _ in 0..self.0.min(n) {
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

fn eval(eng: &mut Engine, threads: usize) -> defuse::Report {
    if threads == 1 {
        eng.evaluate_sequential()
    } else {
        eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256), &mut Threads(threads))
    }
    .expect("evaluate")
}

#[test]
fn counter_folds_give_the_sequential_values_and_guessing_cuts_the_chain() {
    let want = reference(None);
    let mut paths = Vec::new();
    for guess in [false, true] {
        for threads in [1usize, 4, 8] {
            let (mut eng, ops, ids, op) = build(guess);
            let r = eval(&mut eng, threads);
            assert_eq!(seen(&eng, &ids), want, "guess {guess}, j{threads}: cold");
            let cold_runs = ops.runs.load(Relaxed);
            // One guess and one exact run per piece at most.
            assert!(cold_runs <= 2 * N as u64 + 8, "guess {guess}, j{threads}: {cold_runs} runs: {r:?}");
            if threads > 1 {
                paths.push((guess, threads, r.realised_critical_path, cold_runs, r.mispredictions));
            }
            // A write changes: the readers after it see the new counts.
            let k = N / 3;
            let a = {
                let v = eng.values();
                vec![v.int(i64::try_from(k).unwrap()), v.int(1)]
            };
            eng.replace(ids[k], op, a);
            let before = ops.runs.load(Relaxed);
            let r = eval(&mut eng, threads);
            assert_eq!(seen(&eng, &ids), reference(Some(k)), "guess {guess}, j{threads}: edit");
            let edit_runs = ops.runs.load(Relaxed) - before;
            assert!(edit_runs <= 2 * (N - k) as u64 + 8, "guess {guess}, j{threads}: edit {edit_runs} runs: {r:?}");
            assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
        }
    }
    eprintln!("(guess, threads, realised critical path, cold runs, mispredictions): {paths:?}");
    // Guessing breaks the chain through the writers: the realised chain is
    // a few waves, not one link per piece.
    for &(guess, threads, path, _, _) in &paths {
        if guess {
            assert!(path < 16, "j{threads}: critical path {path} with guessed folds: {paths:?}");
        }
    }
}
