//! Quick timing of the bench client's cold build (scratch; not a bench).
#![allow(clippy::pedantic, dead_code)]
use phi::{Arg, Args, Class, ElemId, Graph, Lang, Seq, Step, StepCx, Value, Ver};
use std::time::Instant;

/// A minimal client: integers, versioned by value.
#[derive(Clone, Default, Debug)]
enum V {
    #[default]
    Nil,
    I(i64),
    /// (boxed: a value is 16 bytes)
    S(Box<Seq<V>>),
}

impl Value for V {
    fn ver(&self) -> Ver {
        match self {
            V::Nil => Ver(0),
            V::I(i) => Ver(1 << 64 | u128::from(*i as u64)),
            V::S(s) => s.ver(),
        }
    }
}

impl V {
    fn i(&self) -> i64 {
        match self {
            V::I(i) => *i,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
enum O {
    #[default]
    Nop,
    Add,
    /// An unfold: each step consumes one element and emits ten (twenty)
    /// leaves, the first reading name `acc` (defined by the first step
    /// only, so steps do not chain), the last defining name `out`.
    Para10,
    Para20,
    Para1,
    /// A scan: greedy line filling (converges soon after an edit).
    Mod,
}

struct B;

impl Lang for B {
    type Val = V;
    type Op = O;
    fn eval(op: O, a: &Args<'_, Self>) -> V {
        match op {
            O::Add => V::I((0..a.len()).map(|i| a.get(i).i()).sum::<i64>() & 0xffff),
            _ => V::Nil,
        }
    }
    fn step(op: O, st: &V, _a: &Args<'_, Self>, cx: &mut StepCx<'_, Self>) -> Step<V> {
        let k = match op {
            O::Para10 => 10,
            O::Para20 => 20,
            O::Para1 => 1,
            _ => unreachable!(),
        };
        let Some(x) = cx.next().cloned() else {
            return Step::Done(st.clone());
        };
        let first = cx.consumed() == 1 && matches!(st, V::Nil);
        let solo = cx.name(b"solo");
        if first {
            // (the first step also defines `solo`, which one step reads)
            let c = cx.lit(x.clone());
            cx.define(solo, Arg::Local(c), false);
        }
        let acc = if matches!(x, V::I(-1)) {
            solo
        } else {
            cx.name(b"acc")
        };
        let out = cx.name(b"out");
        let c = cx.lit(x);
        let mut last = cx.leaf(O::Add, Class::Pure, &[Arg::Name(acc), Arg::Local(c)]);
        for _ in 1..k {
            last = cx.leaf(O::Add, Class::Pure, &[Arg::Local(last), Arg::Local(c)]);
        }
        if first {
            // (a constant: no step's reads of `acc` change with the input)
            let k = cx.lit(V::I(7));
            cx.define(acc, Arg::Local(k), false);
        }
        cx.define(out, Arg::Local(last), false);
        let st = V::I(1);
        let key = cx.cursor().0;
        Step::Next {
            st: st.clone(),
            key,
        }
    }
    fn scan(_op: O, st: &V, x: &V, _a: &Args<'_, Self>) -> (V, V) {
        let s = if st.i() + x.i() > 400 {
            x.i()
        } else {
            st.i() + x.i()
        };
        (V::I(s), V::I(s))
    }
    fn as_seq(v: &V) -> Option<&Seq<V>> {
        match v {
            V::S(s) => Some(&**s),
            _ => None,
        }
    }
    fn chain_val(items: Seq<V>) -> V {
        V::S(Box::new(items))
    }
    fn op_tag(op: O) -> u64 {
        match op {
            O::Nop => 0,
            O::Add => 1,
            O::Para10 => 2,
            O::Para20 => 4,
            O::Para1 => 5,
            O::Mod => 3,
        }
    }
    fn entries(op: O, input: &V, _a: &Args<'_, Self>) -> Vec<phi::Entry<V>> {
        let (O::Para10 | O::Para20 | O::Para1, V::S(s)) = (op, input) else {
            return Vec::new();
        };
        // (every 64 elements)
        (64..s.len())
            .step_by(64)
            .map(|i| phi::Entry {
                at: i,
                key: s.get(i).map_or(0, |e| e.0.0),
                guess: Some(V::I(1)),
            })
            .collect()
    }
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(100_000);
    let w: usize = std::env::args()
        .nth(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1);
    let s: Seq<V> = Seq::from_vec(
        (0..n)
            .map(|i| (ElemId(i as u64 + 1), V::I((i * 7919 % 101) as i64)))
            .collect(),
    );
    let mut best = f64::MAX;
    let runs: usize = std::env::var("RUNS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(7);
    for _ in 0..runs {
        let mut g: Graph<B> = Graph::new();
        g.cfg.workers = w;
        if std::env::var("RESERVE").is_ok() {
            g.reserve(2 * n + 16, 4 * n);
        }
        let input = g.input(V::S(Box::new(s.clone())));
        let init = g.input(V::Nil);
        let t = Instant::now();
        g.unfold(
            if std::env::var("K20").is_ok() {
                O::Para20
            } else if std::env::var("K1").is_ok() {
                O::Para1
            } else {
                O::Para10
            },
            input,
            init,
            &[],
        );
        g.run();
        best = best.min(t.elapsed().as_secs_f64());
        let (_, b) = g.mem();
        if w == 1 {
            eprintln!("{:.1} B/node", b as f64 / (n * 12) as f64);
        }
    }
    println!(
        "{n} steps, {w} workers: {:.1} ms, {:.1} ns/node",
        best * 1e3,
        best * 1e9 / (n * 12) as f64
    );
}
