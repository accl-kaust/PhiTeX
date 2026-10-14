//! Toy after Phi's D: loop-back key fields defined later and read earlier
//! (seeded with the previous run's values), count fields published by
//! pieces and read back, a planner folding the texts' contributions, and
//! environment reads answered by a provider.
//!
//! Program (one unit): 200 `["x", i, t]`; after every fifth a piece
//! `["p", j]`; at the end the planner `["l"]`.
//! - `x i` publishes loop-back keys `k{i}_{m}` = `10 t + m` for
//!   `m < i % 4`; reads `k{j}_0` for `j` in `i+3, i+7, i+11` (the first
//!   `i % 4` of those below 200 that publish one); `x` with `i % 10 == 3`
//!   reads the count `c{(i / 5 + 2) % 40}`; it reads one or two
//!   environment files; it contributes `t` to `len`.
//! - `p j` publishes the count `c{j}` = the fold of `len` before it.
//! - `l` outputs the fold of `len`.
//!
//! The client's call pattern: every loop field declared again (same
//! initial value) at push, on edit and whenever the evaluation absorbs
//! edits; the full environment map and absent set set again then, and
//! twice at build start.
//!
//! Asserts: cold, seeded with a previous run's values, takes at most 3
//! rounds with no node run more than 3 times, and every provider answer
//! is counted; a one-value edit runs exactly the edited node and the
//! nodes whose read values changed, once each, with no re-run for the
//! environment.

#![allow(clippy::many_single_char_names)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Rerun, Stop, Value, ValueId, Values};

const X: usize = 200;
const P: usize = 40;

fn tag(v: &Values, a: &[ValueId]) -> String {
    match &*v.get(a[0]) {
        Value::Str(s) => s.to_string(),
        _ => String::new(),
    }
}

fn key(v: &Values, i: usize, m: usize) -> Field {
    v.field(&format!("k{i}_{m}"))
}

fn count(v: &Values, j: usize) -> Field {
    v.field(&format!("c{j}"))
}

/// The keys `x i` reads.
fn key_reads(i: usize) -> Vec<usize> {
    [i + 3, i + 7, i + 11]
        .into_iter()
        .filter(|&j| j < X && j % 4 > 0)
        .take(i % 4)
        .collect()
}

fn count_read(i: usize) -> Option<usize> {
    (i % 10 == 3).then_some((i / 5 + 2) % P)
}

fn files(i: usize) -> Vec<String> {
    let mut f = vec![format!("file{}", i % 13)];
    if i.is_multiple_of(2) {
        f.push(format!("file{}", (i + 5) % 13));
    }
    f
}

/// The node index of each kind (for the run counts).
fn ix_x(i: usize) -> usize {
    i
}
fn ix_p(j: usize) -> usize {
    X + j
}
const IX_L: usize = X + P;

struct Toy {
    runs: Vec<AtomicU64>,
    /// What each node read on its last run.
    seen: Mutex<BTreeMap<usize, Vec<Option<ValueId>>>>,
}

impl Operation for Toy {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let len = v.field("len");
        match tag(v, &a).as_str() {
            "x" => {
                let i = usize::try_from(v.as_int(a[1]).unwrap()).unwrap();
                let t = v.as_int(a[2]).unwrap();
                let mut seen = Vec::new();
                for f in files(i) {
                    seen.push(cx.use_env(v.field(&f))?);
                }
                for j in key_reads(i) {
                    seen.push(cx.use_field(key(v, j, 0))?);
                }
                if let Some(j) = count_read(i) {
                    seen.push(cx.use_field(count(v, j))?);
                }
                self.runs[ix_x(i)].fetch_add(1, Relaxed);
                self.seen.lock().unwrap().insert(ix_x(i), seen.clone());
                for m in 0..i % 4 {
                    cx.publish(key(v, i, m), v.int(10 * t + i64::try_from(m).unwrap()));
                }
                cx.contribute(len, a[2]);
                let s: i64 = seen.iter().map(|x| x.and_then(|x| v.as_int(x)).unwrap_or(0)).sum();
                Ok(v.int(s + t))
            }
            "p" => {
                let j = usize::try_from(v.as_int(a[1]).unwrap()).unwrap();
                let at = cx.acc_range(len, None)?;
                self.runs[ix_p(j)].fetch_add(1, Relaxed);
                self.seen.lock().unwrap().insert(ix_p(j), vec![Some(at)]);
                cx.publish(count(v, j), at);
                Ok(at)
            }
            _ => {
                let at = cx.acc_range(len, None)?;
                self.runs[IX_L].fetch_add(1, Relaxed);
                self.seen.lock().unwrap().insert(IX_L, vec![Some(at)]);
                Ok(at)
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

/// The client between rounds: every loop field declared again, the full
/// environment set again.
struct Client {
    seed: Vec<(String, i64)>,
    answers: Arc<Mutex<BTreeMap<String, i64>>>,
    absorbs: u64,
}

impl Client {
    fn again(&mut self, eng: &mut Engine) {
        self.absorbs += 1;
        for (name, x) in &self.seed {
            let v = eng.values();
            let (f, x) = (v.field(name), v.int(*x));
            eng.declare_loopback(f, x);
        }
        let env: Vec<(Field, ValueId)> = {
            let v = eng.values();
            let mut e = vec![(v.field("base"), v.int(1))];
            e.extend(
                self.answers
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(k, x)| (v.field(k), v.int(*x))),
            );
            e
        };
        eng.set_environment(env);
        let absent = vec![eng.values().field("nofile")];
        eng.set_environment_absent(absent);
    }
}

impl defuse::EditSource for Client {
    fn absorb(&mut self, eng: &mut Engine) -> Option<u64> {
        self.again(eng);
        None
    }
}

fn eval(eng: &mut Engine, sched: &str, c: &mut Client) -> defuse::Report {
    match sched {
        "seq" => eng.evaluate_absorbing(&mut defuse::Greedy::default(), &mut defuse::Sequential, c),
        "spec" => eng.evaluate_absorbing(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
            &mut defuse::Sequential,
            c,
        ),
        "random" => eng.evaluate_absorbing(&mut defuse::RandomScheduler::new(11, true), &mut defuse::Sequential, c),
        _ => eng.evaluate_absorbing(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
            &mut Threads,
            c,
        ),
    }
    .expect("evaluate")
}

fn text(i: usize) -> i64 {
    i64::try_from(i % 9 + 1).unwrap()
}

/// The loop-back values a run of texts `t` settles on.
fn truth(t: &[i64]) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    for (i, &ti) in t.iter().enumerate() {
        for m in 0..i % 4 {
            out.push((format!("k{i}_{m}"), 10 * ti + i64::try_from(m).unwrap()));
        }
    }
    let mut acc = 0;
    for (i, &ti) in t.iter().enumerate() {
        acc += ti;
        if i % 5 == 4 {
            out.push((format!("c{}", i / 5), acc));
        }
    }
    out
}

struct Built {
    eng: Engine,
    toy: Arc<Toy>,
    op: defuse::OpId,
    xs: Vec<NodeId>,
    calls: Arc<AtomicU64>,
    client: Client,
}

fn build(seed: &[(String, i64)]) -> Built {
    let mut eng = Engine::new();
    let toy = Arc::new(Toy {
        runs: (0..=IX_L).map(|_| AtomicU64::new(0)).collect(),
        seen: Mutex::new(BTreeMap::new()),
    });
    let op = eng.register(toy.clone());
    let len = eng.values().field("len");
    eng.declare_accumulator(len, Arc::new(defuse::SumMonoid));
    for (name, x) in seed {
        let v = eng.values();
        let (f, x) = (v.field(name), v.int(*x));
        eng.declare_loopback(f, x);
    }
    let calls = Arc::new(AtomicU64::new(0));
    let c2 = calls.clone();
    let answers = Arc::new(Mutex::new(BTreeMap::new()));
    let a2 = answers.clone();
    eng.set_env_provider(Box::new(move |v: &Values, f: Field| {
        c2.fetch_add(1, Relaxed);
        let name = v.as_str(f.0).unwrap_or_default();
        let x = i64::try_from(name.len()).unwrap();
        a2.lock().unwrap().insert(name, x);
        Some(Some(v.int(x)))
    }));
    let mut client = Client {
        seed: seed.to_vec(),
        answers,
        absorbs: 0,
    };
    client.again(&mut eng);
    client.again(&mut eng);
    let u = eng.new_unit();
    let mut xs = Vec::new();
    for i in 0..X {
        // Its keys declared as it is pushed (the client declares the
        // fields a text names when it makes the text), seeded again.
        for (name, x) in seed.iter().filter(|e| e.0.starts_with(&format!("k{i}_"))) {
            let v = eng.values();
            let (f, x) = (v.field(name), v.int(*x));
            eng.declare_loopback(f, x);
        }
        let a = {
            let v = eng.values();
            vec![v.str("x"), v.int(i64::try_from(i).unwrap()), v.int(text(i))]
        };
        xs.push(eng.push(u, op, a));
        if i % 5 == 4 {
            let a = {
                let v = eng.values();
                vec![v.str("p"), v.int(i64::try_from(i / 5).unwrap())]
            };
            eng.push(u, op, a);
        }
    }
    let a = vec![eng.values().str("l")];
    eng.push(u, op, a);
    Built {
        eng,
        toy,
        op,
        xs,
        calls,
        client,
    }
}

fn runs(toy: &Toy) -> Vec<u64> {
    toy.runs.iter().map(|r| r.swap(0, Relaxed)).collect()
}

#[test]
fn loop_keys_counts_and_the_environment() {
    let t0: Vec<i64> = (0..X).map(text).collect();
    let seed = truth(&t0);
    for sched in ["seq", "spec", "random", "par"] {
        let mut b = build(&seed);
        let r = eval(&mut b.eng, sched, &mut b.client);
        let cold = runs(&b.toy);
        eprintln!(
            "{sched} cold: rounds {} runs {} max per node {} env_provided {} calls {} nodes_run {} absorbs {}",
            r.rounds,
            r.runs,
            cold.iter().max().unwrap(),
            r.env_provided,
            b.calls.load(Relaxed),
            r.nodes_run,
            b.client.absorbs
        );
        assert!(r.rounds <= 3, "{sched}: {r:?}");
        assert!(cold.iter().all(|&n| (1..=3).contains(&n)), "{sched}: {cold:?}");
        assert_eq!(r.env_provided, b.calls.load(Relaxed), "{sched}: env_provided");
        assert_eq!(r.env_provided, 13, "{sched}: one answer per file");
        assert_eq!(r.nodes_run, u64::try_from(IX_L + 1).unwrap(), "{sched}: nodes_run");
        // A one-value edit: x 42's text 7 -> 2.
        let e = 42;
        let before = b.toy.seen.lock().unwrap().clone();
        let a = {
            let v = b.eng.values();
            vec![v.str("x"), v.int(i64::try_from(e).unwrap()), v.int(2)]
        };
        b.eng.replace(b.xs[e], b.op, a);
        // The client declares every loop field again (seeded with the
        // previous run's values) and sets the environment again.
        b.client.again(&mut b.eng);
        let r = eval(&mut b.eng, sched, &mut b.client);
        let got = runs(&b.toy);
        let after = b.toy.seen.lock().unwrap().clone();
        let want: Vec<usize> = (0..=IX_L)
            .filter(|&n| n == ix_x(e) || before.get(&n) != after.get(&n))
            .collect();
        let ran: Vec<usize> = (0..=IX_L).filter(|&n| got[n] > 0).collect();
        let causes = |k: fn(&Rerun) -> bool| r.rerun_causes.iter().filter(|c| k(&c.1)).count();
        eprintln!(
            "{sched} edit: ran {} want {} rounds {} env {} loop {} range {}",
            ran.len(),
            want.len(),
            r.rounds,
            causes(|c| matches!(c, Rerun::Environment { .. })),
            causes(|c| matches!(c, Rerun::Loop { .. })),
            causes(|c| matches!(c, Rerun::Range { .. })),
        );
        assert_eq!(ran, want, "{sched}: the nodes that ran");
        assert!(want.iter().all(|&n| got[n] == 1), "{sched}: each once: {got:?}");
        assert_eq!(causes(|c| matches!(c, Rerun::Environment { .. })), 0, "{sched}");
        assert_eq!(b.calls.load(Relaxed), 13, "{sched}: no provider call on the edit");
    }
}
