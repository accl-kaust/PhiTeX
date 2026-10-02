//! Measure the runtime on stub programs:
//!
//! ```text
//! scripts/sandbox cargo run --release -p partex-incr --example measure [lines] [work]
//! ```
//!
//! Two programs of about `lines` statements (default 10000): a random one
//! (every variable live everywhere, forcing points everywhere: adversarial
//! for rounds) and a document-shaped one (paragraphs of scratch
//! computation, section and equation counters, forward references,
//! allocations, rare forcing). `work` (default 0) is the cost of a
//! paragraph's `work` statement; try 2000. For each: a plain run, a
//! recorded build, one-line edits rebuilt incrementally against from
//! scratch, and parallel rounds against the plain run on 1..N threads.

#![allow(clippy::many_single_char_names)]

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use partex_incr::build::{self, Build};
use partex_incr::rounds::{self, Holes};
use partex_incr::stub::generate::{self, Rng, Shape};
use partex_incr::stub::{Program, oracle};
use partex_incr::{Sequential, Threads, run_plain};

fn time<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let t = Instant::now();
    let r = f();
    (r, t.elapsed())
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// Paragraphs of about ten statements separated by blank lines.
fn document(r: &mut Rng, lines: usize, work: u32) -> String {
    let mut s = String::from("set x0 0\nset x1 0\n");
    let mut i = 0;
    while s.lines().count() < lines {
        if i % 20 == 0 {
            let _ = write!(
                s,
                "section\ninc x0\nset x1 0\nemit x0\nlabel L{} x0\n",
                (i / 20) % 16
            );
        }
        for k in 0..3 {
            let _ = writeln!(s, "set x{} {}", 10 + k, r.below(9));
        }
        let _ = write!(
            s,
            "add x10 x11\nmul x12 3\nadd x12 x10\nemit x12\nsay paragraph {i}\n"
        );
        if work > 0 {
            let _ = writeln!(s, "work {work}");
        }
        let _ = write!(s, "inc x1\nemit x1\nref L{}\nalloc\n", r.below(16));
        if r.below(30) == 0 {
            s.push_str("if x0 > 3 then say late\n");
        }
        s.push('\n');
        i += 1;
    }
    s
}

/// A recorded build with ordered maps against flat recording.
fn recording_cost(p: &Program, plain: Duration) {
    let maps = build::Config {
        flat: false,
        ..build::Config::default()
    };
    let recorded = |cfg: &build::Config| {
        median(
            (0..5)
                .map(|_| time(|| Build::new(p.machine(), cfg)).1)
                .collect(),
        )
    };
    let (m, f) = (recorded(&maps), recorded(&build::Config::default()));
    println!(
        "recording, maps / flat     {:9.3} / {:.3} ms  ({:.1}x / {:.1}x a plain run)",
        ms(m),
        ms(f),
        m.as_secs_f64() / plain.as_secs_f64(),
        f.as_secs_f64() / plain.as_secs_f64()
    );
}

fn bench(name: &str, mut p: Program, shape: &Shape, r: &mut Rng) {
    let want = oracle(&p);
    println!(
        "\n== {name}: {} lines, output {} bytes",
        p.len(),
        want.len()
    );
    let plain = median(
        (0..5)
            .map(|_| {
                let ((_, out), d) = time(|| run_plain(p.machine()));
                assert_eq!(out, want);
                d
            })
            .collect(),
    );
    println!("plain run                  {:9.3} ms", ms(plain));

    recording_cost(&p, plain);
    let cfg = build::Config::default();
    let (mut b, cold) = time(|| Build::new(p.machine(), &cfg));
    let (out, link) = time(|| b.output(&Sequential));
    assert_eq!(out, want);
    println!(
        "recorded build             {:9.3} ms  ({} regions; link {:.3} ms)",
        ms(cold),
        b.stats.regions,
        ms(link)
    );

    // One-line edits at random places: incremental against from scratch.
    let mut inc = Vec::new();
    let mut full = Vec::new();
    let mut cost = Vec::new();
    for _ in 0..30 {
        let ids = p.ids();
        let id = ids[r.below(ids.len())];
        let changed = p.replace(id, &generate::statement(r, shape));
        let ((), d) = time(|| b.rebuild(p.machine(), &changed, &cfg));
        let (out, l) = time(|| b.output(&Sequential));
        inc.push(d + l);
        cost.push(b.stats.executed_cost);
        let (fresh, d) = time(|| Build::new(p.machine(), &cfg));
        let (out2, l) = time(|| fresh.output(&Sequential));
        full.push(d + l);
        assert_eq!(out, out2);
    }
    assert_eq!(b.output(&Sequential), oracle(&p));
    cost.sort_unstable();
    let (inc, full) = (median(inc), median(full));
    println!(
        "one-line edit, incremental {:9.3} ms median, link included (cost run: median {}, max {})",
        ms(inc),
        cost[cost.len() / 2],
        cost[cost.len() - 1]
    );
    println!(
        "one-line edit, recorded from scratch {:.3} ms: {:.0}x; plain run {:.3} ms: {:.1}x",
        ms(full),
        full.as_secs_f64() / inc.as_secs_f64(),
        ms(plain),
        plain.as_secs_f64() / inc.as_secs_f64()
    );

    // Parallel rounds against the plain run.
    let want = oracle(&p);
    let max = Threads::available().0;
    let mut n = 1;
    while n <= max {
        for holes in [Holes::Speculate, Holes::Suspend, Holes::Off] {
            let cfg = rounds::Config {
                parts: 4 * n.max(2),
                holes,
                ..rounds::Config::default()
            };
            let mut stats = None;
            let d = median(
                (0..3)
                    .map(|_| {
                        let (o, d) = time(|| {
                            let o = rounds::run(&p.machine(), &cfg, &Threads(n));
                            let out = o.output(&Threads(n));
                            (o, out)
                        });
                        assert_eq!(o.1, want);
                        stats = Some(o.0.stats);
                        d
                    })
                    .collect(),
            );
            let s = stats.expect("ran");
            println!(
                "rounds {n:2} threads {holes:<9?} {:9.3} ms  {:5.2}x plain  ({} rounds, {} runs of {} regions, serial cost {}%{})",
                ms(d),
                plain.as_secs_f64() / d.as_secs_f64(),
                s.rounds,
                s.runs,
                s.regions,
                100 * s.serial_cost / (s.serial_cost + s.parallel_cost).max(1),
                if s.fallback { ", fallback" } else { "" }
            );
        }
        n = if n == max { max + 1 } else { (n * 2).min(max) };
    }
    warm_starts(&mut p, shape, r, max);
}

/// Warm starts: rounds seeded with the previous run's regions, after a
/// one-line edit, against a cold start of the edited program.
fn warm_starts(p: &mut Program, shape: &Shape, r: &mut Rng, n: usize) {
    for holes in [Holes::Speculate, Holes::Suspend] {
        let cfg = rounds::Config {
            parts: 4 * n.max(2),
            holes,
            ..rounds::Config::default()
        };
        let mut cold_t = Vec::new();
        let mut warm_t = Vec::new();
        let mut cold_s = None;
        let mut warm_s = None;
        for _ in 0..9 {
            let before = rounds::run(&p.machine(), &cfg, &Threads(n));
            let warm = rounds::Warm::from_outcome(&before);
            let ids = p.ids();
            let id = ids[r.below(ids.len())];
            p.replace(id, &generate::statement(r, shape));
            let want = oracle(p);
            let (o, d) = time(|| {
                let o = rounds::run(&p.machine(), &cfg, &Threads(n));
                let out = o.output(&Threads(n));
                (o, out)
            });
            assert_eq!(o.1, want);
            cold_t.push(d);
            cold_s = Some(o.0.stats);
            let (o, d) = time(|| {
                let o = rounds::run_warm(&p.machine(), &cfg, &Threads(n), Some(&warm));
                let out = o.output(&Threads(n));
                (o, out)
            });
            assert_eq!(o.1, want);
            warm_t.push(d);
            warm_s = Some(o.0.stats);
        }
        let (c, w) = (median(cold_t), median(warm_t));
        let (cs, ws) = (cold_s.expect("ran"), warm_s.expect("ran"));
        println!(
            "rounds {n:2} threads {holes:<9?} after an edit: cold {:9.3} ms ({} rounds, {} runs), warm {:9.3} ms ({} rounds, {} runs): {:.1}x",
            ms(c),
            cs.rounds,
            cs.runs,
            ms(w),
            ws.rounds,
            ws.runs,
            c.as_secs_f64() / w.as_secs_f64()
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let lines: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let work: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let shape = Shape {
        vars: 48,
        labels: 16,
        work,
        raw: false,
        acc: false,
    };
    let mut r = Rng(42);
    let random = Program::from_text(&generate::program(&mut r, lines, &shape));
    bench(
        &format!("random program, work <= {work}"),
        random,
        &shape,
        &mut r,
    );
    let doc = Program::from_text(&document(&mut r, lines, work));
    bench(
        &format!("document-shaped, work {work} per paragraph"),
        doc,
        &shape,
        &mut r,
    );
}
