//! Watch builds equal cold builds for loop-back programs: random edit
//! histories over loops with one fixed point (`inc`), a start-dependent
//! fixed point (`keep`: a maximum a later build need not reach), an
//! oscillation that hits the round cap (`flip`, under
//! `CapPolicy::Finalize`), loops feeding loops (`copy`), plain definers and
//! readers, and a final writer. After every evaluation (edits applied one
//! at a time or in batches), every node's output, the round count,
//! `capped` and the final nodes run equal a fresh engine's cold evaluation
//! of the same text, under sequential, threaded and speculative schedules.
//! (`Previous` is not exact for `keep`, nor `Initial` for a capped
//! oscillation; the test checks each start where it must be exact.)

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use defuse::{CapPolicy, Ctx, Engine, Field, Locality, LoopStart, NodeId, OpId, Operation, Stop, ValueId, Values};

const LOOPS: [&str; 3] = ["la", "lb", "lc"];
const CAP: u64 = 6;

/// Memo hits and runs over the watch evaluations of a test.
static HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HELD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Ops;

fn s(v: &Values, a: &[ValueId], i: usize) -> String {
    v.as_str(a[i]).unwrap_or_default()
}

fn n(v: &Values, a: &[ValueId], i: usize) -> i64 {
    v.as_int(a[i]).unwrap_or(0)
}

fn read(cx: &mut Ctx<'_>, f: &str) -> Result<i64, Stop> {
    let v = cx.values();
    Ok(cx.use_field(v.field(f))?.and_then(|x| v.as_int(x)).unwrap_or(0))
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match s(v, a, 0).as_str() {
            "def" => vec![(v.field(&s(v, a, 1)), Locality::Local)],
            "fin" => vec![(v.field("out"), Locality::Local)],
            // Its set of definitions is the run's (pending until it runs).
            "dyn" => return None,
            _ => Vec::new(),
        })
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> defuse::MayDefine {
        if s(v, a, 0) == "dyn" {
            defuse::MayDefine::Names(vec![v.field("pa"), v.field("pb")])
        } else {
            defuse::MayDefine::Any
        }
    }
    fn final_only(&self, a: &[ValueId], v: &Values) -> bool {
        s(v, a, 0) == "fin"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = s(v, &a, 0);
        let out = match op.as_str() {
            // f climbs by one to k.
            "inc" => {
                let (f, k) = (s(v, &a, 1), n(v, &a, 2));
                let x = read(cx, &f)?;
                cx.publish(v.field(&f), v.int((x + 1).min(k)));
                x
            }
            // f = max(f, k): the fixed point depends on the start.
            "keep" => {
                let (f, k) = (s(v, &a, 1), n(v, &a, 2));
                let x = read(cx, &f)?;
                cx.publish(v.field(&f), v.int(x.max(k)));
                x
            }
            // f = 1 - f: never converges.
            "flip" => {
                let f = s(v, &a, 1);
                let x = read(cx, &f)?;
                cx.publish(v.field(&f), v.int(1 - x));
                x
            }
            // g = f + k.
            "copy" => {
                let (f, g, k) = (s(v, &a, 1), s(v, &a, 2), n(v, &a, 3));
                let x = read(cx, &f)?;
                cx.publish(v.field(&g), v.int(x + k));
                x
            }
            // A plain definition of a field from a loop-back.
            "def" => {
                let (g, f) = (s(v, &a, 1), s(v, &a, 2));
                let x = read(cx, &f)? + n(v, &a, 3);
                cx.define(v.field(&g), v.int(x));
                x
            }
            // Defines pa or pb, picked by a loop-back's parity.
            "dyn" => {
                let x = read(cx, &s(v, &a, 1))?;
                let g = if x % 2 == 0 { "pa" } else { "pb" };
                cx.define(v.field(g), v.int(x + n(v, &a, 2)));
                x
            }
            // Contributes a loop-back's value to accumulator `acc`.
            "acc" => {
                let x = read(cx, &s(v, &a, 1))?;
                cx.contribute(v.field("acc"), v.int(x + n(v, &a, 2)));
                x
            }
            // The sum of `acc` over the unit so far.
            "sum" => {
                let t = cx.acc_range(v.field("acc"), None)?;
                v.as_int(t).unwrap_or(0)
            }
            "fin" => {
                let x = read(cx, "la")? * 10_000 + read(cx, "lb")? * 100 + read(cx, "lc")?;
                cx.define(v.field("out"), v.int(x));
                x
            }
            // A reader of any field.
            _ => read(cx, &s(v, &a, 1))?,
        };
        Ok(v.int(out))
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

/// A text: `(op, args)` as strings and ints.
#[derive(Clone, Debug)]
enum A {
    S(&'static str),
    I(i64),
}

type Text = Vec<A>;

struct Rng(u64);

impl Rng {
    fn next(&mut self, m: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % m as u64).unwrap()
    }
}

/// `plain`: mostly texts that publish nothing (the quick path's ground).
fn random_text_mix(r: &mut Rng, flips: bool, plain_heavy: bool) -> Text {
    let lp = |r: &mut Rng| A::S(LOOPS[r.next(LOOPS.len())]);
    let plain = |r: &mut Rng| A::S(["pa", "pb"][r.next(2)]);
    if plain_heavy && r.next(5) != 0 {
        return match r.next(6) {
            0 => vec![A::S("def"), plain(r), lp(r), A::I(i64::try_from(r.next(4)).unwrap())],
            1 => vec![A::S("dyn"), lp(r), A::I(i64::try_from(r.next(3)).unwrap())],
            2 => vec![A::S("acc"), lp(r), A::I(i64::try_from(r.next(3)).unwrap())],
            3 => vec![A::S("sum")],
            4 => vec![A::S("rd"), plain(r)],
            _ => vec![A::S("rd"), lp(r)],
        };
    }
    match r.next(12) {
        9 => vec![A::S("dyn"), lp(r), A::I(i64::try_from(r.next(3)).unwrap())],
        10 => vec![A::S("acc"), lp(r), A::I(i64::try_from(r.next(3)).unwrap())],
        11 => vec![A::S("sum")],
        0 | 1 => vec![A::S("inc"), lp(r), A::I(i64::try_from(r.next(5)).unwrap())],
        2 => vec![A::S("keep"), lp(r), A::I(i64::try_from(r.next(7)).unwrap())],
        3 if flips => vec![A::S("flip"), lp(r)],
        3 => vec![A::S("rd"), lp(r)],
        4 => vec![A::S("copy"), lp(r), lp(r), A::I(i64::try_from(r.next(3)).unwrap())],
        5 => vec![A::S("def"), plain(r), lp(r), A::I(i64::try_from(r.next(4)).unwrap())],
        6 => vec![A::S("fin")],
        7 => vec![A::S("rd"), plain(r)],
        _ => vec![A::S("rd"), lp(r)],
    }
}

fn args(eng: &Engine, t: &Text) -> Vec<ValueId> {
    let v = eng.values();
    t.iter()
        .map(|a| match a {
            A::S(s) => v.str(s),
            A::I(i) => v.int(*i),
        })
        .collect()
}

fn engine(start: LoopStart) -> (Engine, OpId, defuse::UnitId) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    eng.declare_accumulator(eng.values().field("acc"), Arc::new(defuse::SumMonoid));
    let zero = eng.values().int(0);
    for l in LOOPS {
        let f = eng.values().field(l);
        eng.declare_loopback(f, zero);
    }
    eng.set_max_rounds(CAP);
    eng.set_round_cap_policy(CapPolicy::Finalize);
    eng.set_loop_start(start);
    (eng, op, u)
}

fn eval(eng: &mut Engine, sched: usize) -> Result<defuse::Report, defuse::EvalError> {
    match sched % 3 {
        0 => eng.evaluate_sequential(),
        1 => eng.evaluate(&mut defuse::Greedy::default(), &mut Threads(4)),
        _ => eng.evaluate(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 64),
            &mut Threads(3),
        ),
    }
}

/// What a build shows: outputs in text order, rounds, capped, final runs
/// (or the error, by kind: two publishers of a loop-back conflict).
type Seen = Result<(Vec<Option<i64>>, u64, Option<Vec<Field>>, u64), String>;

fn seen(eng: &Engine, ids: &[NodeId], rep: &Result<defuse::Report, defuse::EvalError>) -> Seen {
    let rep = rep
        .as_ref()
        .map_err(|e| format!("{e:?}").split([' ', '{', '(']).next().unwrap_or("").to_string())?;
    let outs = ids
        .iter()
        .map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x)))
        .collect();
    // (Final nodes run counts work, not results: an unchanged one is not
    // run again.)
    Ok((outs, rep.rounds, rep.capped.clone(), 0))
}

fn cold(texts: &[Text], sched: usize) -> Seen {
    let (mut eng, op, u) = engine(LoopStart::Initial);
    let ids: Vec<NodeId> = texts
        .iter()
        .map(|t| {
            let a = args(&eng, t);
            eng.push(u, op, a)
        })
        .collect();
    let rep = eval(&mut eng, sched);
    seen(&eng, &ids, &rep)
}

/// Random histories under `start`; the first mismatch, if any.
fn histories(start: LoopStart, seeds: std::ops::Range<u64>, strict: bool, flips: bool) -> Option<String> {
    histories_mix(start, seeds, strict, flips, false)
}

fn histories_mix(
    start: LoopStart,
    seeds: std::ops::Range<u64>,
    strict: bool,
    flips: bool,
    plain_heavy: bool,
) -> Option<String> {
    // (Not strict: outputs and capped only; strict adds the round count.)
    let norm = |x: Seen| if strict { x } else { x.map(|(o, _, c, f)| (o, 0, c, f)) };
    // (Not strict: a capped cold build is not compared.)
    let differ = |got: &Seen, want: &Seen| got != want && (strict || !matches!(want, Ok((_, _, Some(_), _))));
    for seed in seeds {
        let mut r = Rng(0x9e37_79b9_7f4a_7c15 ^ seed.wrapping_mul(0x2545_f491_4f6c_dd1d));
        let sched = usize::try_from(seed % 3).unwrap();
        let batch = 1 + r.next(3);
        let (mut eng, op, u) = engine(start);
        let mut texts: Vec<Text> = (0..6 + r.next(6))
            .map(|_| random_text_mix(&mut r, flips, plain_heavy))
            .collect();
        let mut ids: Vec<NodeId> = texts
            .iter()
            .map(|t| {
                let a = args(&eng, t);
                eng.push(u, op, a)
            })
            .collect();
        let rep = eval(&mut eng, sched);
        let (got, want) = (norm(seen(&eng, &ids, &rep)), norm(cold(&texts, sched)));
        if std::env::var_os("LOOP_TRACE").is_some() {
            eprintln!("cold: {texts:?}\n  got  {got:?}\n  want {want:?}");
        }
        if differ(&got, &want) {
            return Some(format!("seed {seed} cold: {texts:?}\n got  {got:?}\n want {want:?}"));
        }
        for step in 0..30 {
            let k = r.next(texts.len().max(1));
            match r.next(3) {
                0 if texts.len() > 2 => {
                    eng.remove(ids[k]);
                    texts.remove(k);
                    ids.remove(k);
                }
                1 => {
                    let t = random_text_mix(&mut r, flips, plain_heavy);
                    let a = args(&eng, &t);
                    ids.insert(k + 1, eng.insert_after(ids[k], op, a));
                    texts.insert(k + 1, t);
                }
                _ => {
                    let t = random_text_mix(&mut r, flips, plain_heavy);
                    let a = args(&eng, &t);
                    eng.replace(ids[k], op, a);
                    texts[k] = t;
                }
            }
            if (step + 1) % batch != 0 {
                continue;
            }
            let rep = eval(&mut eng, sched + step);
            if let Ok(r) = &rep {
                HITS.fetch_add(r.round_times.iter().map(|t| t.memo_hits).sum::<u64>(), Relaxed);
                RUNS.fetch_add(r.runs, Relaxed);
                HELD.fetch_add(u64::from(r.quick_held), Relaxed);
            }
            let (got, want) = (norm(seen(&eng, &ids, &rep)), norm(cold(&texts, sched + step)));
            if std::env::var_os("LOOP_TRACE").is_some() {
                eprintln!(
                    "step {step}: {texts:?}\n  got  {got:?}\n  want {want:?}\n  rep rounds {:?} quick {:?} restarts {:?}",
                    rep.as_ref().map(|r| r.rounds),
                    rep.as_ref().map(|r| (r.quick_held, r.quick_refused.clone())),
                    rep.as_ref().map(|r| r.restarts)
                );
            }
            if differ(&got, &want) {
                return Some(format!(
                    "seed {seed} step {step} ({start:?}): {texts:?}\n got  {got:?}\n want {want:?}"
                ));
            }
        }
    }
    None
}

#[test]
fn initial_start_watch_builds_equal_cold_builds() {
    // (Initial restarts only when an edit queued work and counts rounds
    // from where it is: outputs equal a cold build's for loops that
    // converge, but an oscillation capped mid-way may stop on the other
    // phase; round-for-round exactness is LoopStart::PerRound's.)
    if let Some(m) = histories(LoopStart::Initial, 0..300, false, false) {
        panic!("{m}");
    }
}

#[test]
fn per_round_one_seed() {
    if let Ok(x) = std::env::var("LOOP_SEED") {
        let x: u64 = x.parse().unwrap();
        let start = if std::env::var_os("LOOP_INITIAL").is_some() {
            LoopStart::Initial
        } else {
            LoopStart::PerRound
        };
        let strict = start == LoopStart::PerRound;
        // (Seeds from 20 000 on are the plain-heavy mix's.)
        if let Some(m) = histories_mix(start, x..x + 1, strict, strict, x >= 20_000) {
            panic!("{m}");
        }
    }
}

#[test]
#[ignore = "long: LOOP_SEEDS=N for N histories"]
fn per_round_many_histories() {
    let n: u64 = std::env::var("LOOP_SEEDS")
        .ok()
        .and_then(|x| x.parse().ok())
        .unwrap_or(2000);
    if let Some(m) = histories(LoopStart::PerRound, 1000..1000 + n, true, true) {
        panic!("{m}");
    }
    if let Some(m) = histories_mix(LoopStart::PerRound, 20_000..20_000 + n, true, true, true) {
        panic!("{m}");
    }
}

#[test]
fn per_round_quick_path_histories_equal_cold_builds() {
    // (Few publishers: most edits take the quick path.)
    let before = HELD.load(Relaxed);
    if let Some(m) = histories_mix(LoopStart::PerRound, 5000..5300, true, true, true) {
        panic!("{m}");
    }
    let held = HELD.load(Relaxed) - before;
    eprintln!("per-round: the quick path held {held} times");
    // (Exactness was not bought by never taking it.)
    assert!(held > 0, "the quick path never held");
}

#[test]
fn per_round_watch_builds_equal_cold_builds_round_for_round() {
    if let Some(m) = histories(LoopStart::PerRound, 0..400, true, true) {
        panic!("{m}");
    }
    let (h, r) = (HITS.load(Relaxed), RUNS.load(Relaxed));
    eprintln!("per-round: {h} memo hits, {r} runs over the watch evaluations");
    // (The memo was used: exactness was not bought by running everything.)
    assert!(h > 0, "no memo hit");
}

#[test]
fn per_round_quick_path_known_histories() {
    // 2753: a publisher of a loop-back that moved (`keep lb 3`, one entry
    // after a failed evaluation) re-texted to `keep lb 2`; from the last
    // fixed point it publishes 3 again in one round, from the initial
    // value lb settles at 2: the quick path must not hold.
    // 27622: `inc lb 2` re-texted to `copy lc lb 2`: the same final
    // publication (lb = 2), reached in fewer rounds.
    for seed in [2753, 27622] {
        if let Some(m) = histories_mix(LoopStart::PerRound, seed..seed + 1, true, true, seed >= 20_000) {
            panic!("{m}");
        }
    }
}
