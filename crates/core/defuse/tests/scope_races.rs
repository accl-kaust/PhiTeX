//! Scopes under threads: brackets known only by running, opened and
//! closed by nodes that run in any order, read as an environment's name
//! is read at its end. Every thread count gives what one thread gives.

use std::sync::Arc;
use std::time::{Duration, Instant};

use phitex_defuse::{Brackets, Ctx, Effect, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

fn spin(us: u64) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_micros(us) {
        std::hint::spin_loop();
    }
}

fn us_of(v: &Values, x: ValueId) -> u64 {
    v.as_int(x).unwrap_or(0).unsigned_abs()
}

/// `[f, k, us]`: opens a scope after itself and defines f = k inside it,
/// its brackets known only by running (`\begin{k}`).
struct Begin;

impl Operation for Begin {
    fn name(&self) -> &'static str {
        "begin"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Pending
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, k) = (Field(cx.args()[0]), cx.args()[1]);
        spin(us_of(cx.values(), cx.args()[2]));
        cx.define_with(f, k, Locality::Scope);
        cx.set_effect(Effect::Scope(Brackets {
            closes: 0,
            opens: 1,
            before: false,
        }));
        Ok(k)
    }
}

/// `[f, us]`: reads f, then closes the scope it runs in (`\end{k}`
/// checking the environment's name); outputs what it read.
struct End;

impl Operation for End {
    fn name(&self) -> &'static str {
        "end"
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Pending
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let f = Field(cx.args()[0]);
        spin(us_of(cx.values(), cx.args()[1]));
        let x = cx.read(f)?.unwrap_or(ValueId::UNIT);
        cx.set_effect(Effect::Scope(Brackets {
            closes: 1,
            opens: 0,
            before: false,
        }));
        Ok(x)
    }
}

/// `[f, us]`: outputs the definition of f reaching it.
struct Get;

impl Operation for Get {
    fn name(&self) -> &'static str {
        "get"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let f = Field(cx.args()[0]);
        spin(us_of(cx.values(), cx.args()[1]));
        Ok(cx.read(f)?.unwrap_or(ValueId::UNIT))
    }
}

struct Ops {
    begin: OpId,
    end: OpId,
    get: OpId,
}

fn engine() -> (Engine, Ops) {
    let e = Engine::new();
    let o = Ops {
        begin: e.register(Arc::new(Begin)),
        end: e.register(Arc::new(End)),
        get: e.register(Arc::new(Get)),
    };
    (e, o)
}

/// A small generator, the same sequence for the same seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A document of nested environments, `n` nodes: what each node outputs,
/// evaluated on `threads` threads.
fn outputs(seed: u64, n: usize, threads: usize) -> Vec<Option<i64>> {
    let (e, o) = engine();
    let v = e.values();
    let f = v.field("cur").0;
    let u = e.new_unit();
    let mut rng = Rng(seed | 1);
    let mut depth = 0usize;
    let mut k = 0i64;
    let mut ids: Vec<NodeId> = Vec::with_capacity(n);
    for _ in 0..n {
        let us = v.int(i64::try_from(rng.below(60)).unwrap_or(0));
        let r = rng.below(10);
        let id = if r < 3 {
            k += 1;
            depth += 1;
            e.push(u, o.begin, vec![f, v.int(k), us])
        } else if r < 6 && depth > 0 {
            depth -= 1;
            e.push(u, o.end, vec![f, us])
        } else {
            e.push(u, o.get, vec![f, us])
        };
        ids.push(id);
    }
    if threads == 1 {
        e.evaluate_sequential().unwrap();
    } else {
        e.evaluate(threads).unwrap();
    }
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
    ids.iter().map(|&id| e.output(id).and_then(|x| v.as_int(x))).collect()
}

#[test]
fn environments_end_where_they_begin_at_every_thread_count() {
    let threads = std::thread::available_parallelism().map_or(1, usize::from).clamp(2, 16);
    let rounds: u64 = std::env::var("SCOPE_RACE_ROUNDS")
        .ok()
        .and_then(|x| x.parse().ok())
        .unwrap_or(40);
    for seed in 1..=rounds {
        let n = std::env::var("SCOPE_RACE_N")
            .ok()
            .and_then(|x| x.parse().ok())
            .unwrap_or(300);
        let want = outputs(seed, n, 1);
        for t in [threads, 4] {
            let got = outputs(seed, n, t);
            let first = want.iter().zip(&got).position(|(a, b)| a != b);
            assert!(
                first.is_none(),
                "seed {seed} at {t} threads: node {first:?} gives {:?}, one thread {:?}",
                first.map(|i| got[i]),
                first.map(|i| want[i])
            );
        }
    }
}

/// Many readers after many environments that open and close at once: the
/// walks of the readers after each bracket change overlap, and one left
/// for another's walker (`walk::change`) is walked after it.
/// Every thread count gives what one thread gives.
#[test]
fn bracket_changes_left_for_a_walker_are_walked() {
    let threads = std::thread::available_parallelism().map_or(1, usize::from).clamp(4, 16);
    for seed in 1..=8u64 {
        let (want, got) = (dense(seed, 1), dense(seed, threads));
        let first = want.iter().zip(&got).position(|(a, b)| a != b);
        assert!(
            first.is_none(),
            "seed {seed} at {threads} threads: node {first:?} gives {:?}, one thread {:?}",
            first.map(|i| got[i]),
            first.map(|i| want[i])
        );
    }
}

/// `outputs` with ten readers after each environment's bracket, no spin on
/// the readers: many short runs, each bracket change a walk over them.
fn dense(seed: u64, threads: usize) -> Vec<Option<i64>> {
    let (e, o) = engine();
    let v = e.values();
    let f = v.field("cur").0;
    let u = e.new_unit();
    let mut rng = Rng(seed | 1);
    let (mut depth, mut k) = (0usize, 0i64);
    let mut ids: Vec<NodeId> = Vec::new();
    let zero = v.int(0);
    for _ in 0..200 {
        let us = v.int(i64::try_from(rng.below(40)).unwrap_or(0));
        if rng.below(2) == 0 || depth == 0 {
            k += 1;
            depth += 1;
            ids.push(e.push(u, o.begin, vec![f, v.int(k), us]));
        } else {
            depth -= 1;
            ids.push(e.push(u, o.end, vec![f, us]));
        }
        for _ in 0..10 {
            ids.push(e.push(u, o.get, vec![f, zero]));
        }
    }
    if threads == 1 {
        e.evaluate_sequential().unwrap();
    } else {
        e.evaluate(threads).unwrap();
    }
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
    ids.iter().map(|&id| e.output(id).and_then(|x| v.as_int(x))).collect()
}

/// Separate environments, each a begin, readers and an end, far apart:
/// with `PHITEX_PWALK` the walks after their bracket changes own
/// separate spans and may run side by side (`phitex_defuse::walks_most`
/// reports how many did at once). Every thread count gives what one
/// thread gives, in every round.
#[test]
fn separate_environments_walk_side_by_side() {
    let threads = std::thread::available_parallelism().map_or(1, usize::from).clamp(4, 16);
    for seed in 1..=8u64 {
        let (want, got) = (apart(seed, 1), apart(seed, threads));
        let first = want.iter().zip(&got).position(|(a, b)| a != b);
        assert!(
            first.is_none(),
            "seed {seed} at {threads} threads: node {first:?} gives {:?}, one thread {:?}",
            first.map(|i| got[i]),
            first.map(|i| want[i])
        );
    }
    let most = phitex_defuse::walks_most();
    eprintln!("bracket walks at once, most: {most}");
    assert!(
        most > 1,
        "no two bracket walks ran at once in 8 rounds at {threads} threads"
    );
}

/// Twenty environments, each `begin` (slow), forty readers, `end`
/// (fast): an end often commits before its begin, so the begin's region
/// stops at it.
fn apart(seed: u64, threads: usize) -> Vec<Option<i64>> {
    let (e, o) = engine();
    let v = e.values();
    let f = v.field("cur").0;
    let u = e.new_unit();
    let mut rng = Rng(seed | 1);
    let mut ids: Vec<NodeId> = Vec::new();
    for k in 1..=20i64 {
        let us = v.int(i64::try_from(200 + rng.below(400)).unwrap_or(0));
        ids.push(e.push(u, o.begin, vec![f, v.int(k), us]));
        for _ in 0..40 {
            ids.push(e.push(u, o.get, vec![f, v.int(i64::try_from(rng.below(20)).unwrap_or(0))]));
        }
        ids.push(e.push(u, o.end, vec![f, v.int(0)]));
    }
    if threads == 1 {
        e.evaluate_sequential().unwrap();
    } else {
        e.evaluate(threads).unwrap();
    }
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
    ids.iter().map(|&id| e.output(id).and_then(|x| v.as_int(x))).collect()
}
