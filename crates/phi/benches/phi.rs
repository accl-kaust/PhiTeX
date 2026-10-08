//! The φ core's costs (DESIGN 7.11): build and evaluate N nodes, a
//! rebuild after one leaf edit, a scan resume, parallel scaling, and the
//! memory a node takes.

#![allow(clippy::pedantic)]

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use phi::{Arg, Args, Class, ElemId, Graph, Lang, Sel, Seq, Step, StepCx, Value, Ver};

/// A minimal client: integers, versioned by value.
#[derive(Clone, Default, Debug)]
enum V {
    #[default]
    Nil,
    I(i64),
    S(Seq<V>),
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
    /// An unfold: each step consumes one element and emits `k` leaves,
    /// the first reading name `acc` (defined by the first step only, so
    /// steps do not chain), the last defining name `out`.
    Para(u32),
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
        let O::Para(k) = op else { unreachable!() };
        let Some(x) = cx.next().cloned() else {
            return Step::Done(st.clone());
        };
        let first = cx.consumed() == 1 && matches!(st, V::Nil);
        let acc = cx.name(b"acc");
        let out = cx.name(b"out");
        let c = cx.lit(x);
        let mut last = cx.leaf(O::Add, Class::Pure, &[Arg::Name(acc), Arg::Local(c)]);
        for _ in 1..k {
            last = cx.leaf(O::Add, Class::Pure, &[Arg::Local(last), Arg::Local(c)]);
        }
        if first {
            cx.define(acc, Arg::Local(c), false);
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
            V::S(s) => Some(s),
            _ => None,
        }
    }
    fn chain_val(items: Seq<V>) -> V {
        V::S(items)
    }
    fn op_tag(op: O) -> u64 {
        match op {
            O::Nop => 0,
            O::Add => 1,
            O::Para(k) => 2 | u64::from(k) << 8,
            O::Mod => 3,
        }
    }
    fn entries(op: O, input: &V, _a: &Args<'_, Self>) -> Vec<phi::Entry<V>> {
        let (O::Para(_), V::S(s)) = (op, input) else {
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

fn elems(n: usize) -> Seq<V> {
    Seq::from_vec(
        (0..n)
            .map(|i| (ElemId(i as u64 + 1), V::I((i * 7919 % 101) as i64)))
            .collect(),
    )
}

/// N leaves in the root region, a chain over one input.
fn chain(n: usize) -> (Graph<B>, phi::NodeId, phi::NodeId) {
    let mut g: Graph<B> = Graph::new();
    g.reserve(n + 2, 2 * n);
    let x = g.input(V::I(1));
    let mut last = x;
    for _ in 0..n {
        last = g.leaf(O::Add, Class::Pure, &[(last, Sel::WHOLE), (x, Sel::WHOLE)]);
    }
    g.run();
    (g, x, last)
}

/// An unfold of `n` steps emitting `k` leaves each.
fn unfold(n: usize, k: u32, workers: usize) -> (Graph<B>, phi::NodeId, phi::NodeId) {
    unfold_over(elems(n), k, workers)
}

fn unfold_over(s: Seq<V>, k: u32, workers: usize) -> (Graph<B>, phi::NodeId, phi::NodeId) {
    let n = s.len();
    let mut g: Graph<B> = Graph::new();
    g.cfg.workers = workers;
    g.reserve(n * (k as usize + 2) + 8, n * (2 * k as usize + 2));
    let input = g.input(V::S(s));
    let init = g.input(V::Nil);
    let u = g.unfold(O::Para(k), input, init, &[]);
    g.run();
    (g, input, u)
}

fn build(c: &mut Criterion) {
    let mut grp = c.benchmark_group("build");
    grp.sample_size(10);
    for n in [10_000usize, 1_000_000] {
        grp.throughput(criterion::Throughput::Elements(n as u64));
        grp.bench_with_input(BenchmarkId::new("leaves", n), &n, |b, &n| {
            b.iter(|| black_box(chain(n).2))
        });
        // (a step and its constant are nodes too: k + 2 a step)
        let steps = n / 10;
        grp.throughput(criterion::Throughput::Elements((steps * 12) as u64));
        let input = elems(steps);
        grp.bench_with_input(BenchmarkId::new("unfold-k10", n), &steps, |b, _| {
            b.iter(|| black_box(unfold_over(input.clone(), 10, 1).2))
        });
    }
    grp.finish();
}

fn rebuild(c: &mut Criterion) {
    let mut grp = c.benchmark_group("rebuild");
    grp.sample_size(20);
    // one input read by one leaf of a million, each reading its own input
    let n = 1_000_000;
    let mut g: Graph<B> = Graph::new();
    g.reserve(2 * n + 2, 2 * n);
    let mut inputs = Vec::new();
    for i in 0..n {
        let x = g.input(V::I(i as i64));
        let l = g.leaf(O::Add, Class::Pure, &[(x, Sel::WHOLE)]);
        inputs.push((x, l));
    }
    g.run();
    let mut v = 0;
    grp.bench_function("one-leaf-of-1M", |b| {
        b.iter(|| {
            v += 1;
            g.set(inputs[n / 2].0, V::I(v));
            black_box(g.run().evals)
        })
    });
    // a word changed in an unfold of 100k steps (1.2M nodes)
    let (mut g, input, _) = unfold(100_000, 10, 1);
    let base = elems(100_000);
    let mut k = 0i64;
    grp.bench_function("one-step-of-100k", |b| {
        b.iter(|| {
            k += 1;
            let s = base.splice(50_000, 1, [(ElemId(50_001), V::I(k % 50))]);
            g.set(input, V::S(s));
            black_box(g.run().steps)
        })
    });
    grp.finish();
}

fn scan(c: &mut Criterion) {
    let mut grp = c.benchmark_group("scan");
    grp.sample_size(20);
    let n = 1_000_000;
    let mut g: Graph<B> = Graph::new();
    let base = elems(n);
    let input = g.input(V::S(base.clone()));
    let init = g.input(V::I(0));
    g.scan(O::Mod, (input, Sel::WHOLE), init, &[]);
    g.run();
    let mut k = 0i64;
    grp.bench_function("resume-1M", |b| {
        b.iter(|| {
            k += 1;
            let s = base.splice(n / 2, 1, [(ElemId(n as u64 / 2 + 1), V::I(k % 50))]);
            g.set(input, V::S(s));
            black_box(g.run().scanned)
        })
    });
    grp.finish();
}

fn parallel(c: &mut Criterion) {
    let mut grp = c.benchmark_group("parallel");
    grp.sample_size(10);
    let steps = 50_000;
    grp.throughput(criterion::Throughput::Elements((steps * 22) as u64));
    let input = elems(steps);
    for w in [1usize, 2, 4, 8] {
        grp.bench_with_input(BenchmarkId::new("cold-unfold-k20", w), &w, |b, &w| {
            b.iter(|| black_box(unfold_over(input.clone(), 20, w).2))
        });
    }
    grp.finish();
}

fn memory(_c: &mut Criterion) {
    let (g, _, _) = chain(1_000_000);
    let (n, b) = g.mem();
    eprintln!(
        "memory: leaves: {n} nodes, {b} bytes, {:.1} B/node",
        b as f64 / n as f64
    );
    let (g, _, _) = unfold(100_000, 10, 1);
    let (n, b) = g.mem();
    eprintln!(
        "memory: unfold k=10: {n} nodes, {b} bytes, {:.1} B/node",
        b as f64 / n as f64
    );
}

criterion_group!(benches, memory, build, rebuild, scan, parallel);
criterion_main!(benches);
