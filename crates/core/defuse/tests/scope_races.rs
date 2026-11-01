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
