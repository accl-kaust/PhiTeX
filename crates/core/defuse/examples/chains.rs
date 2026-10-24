//! Pending-claim chains: N claimers in a row, each one's extent known only
//! from a value computed from the previous claim's result, so nothing about
//! the structure is known before evaluation and the claims resolve one
//! after another. Two shapes:
//!
//! - `inert`: `listuntil` claims (the taken nodes go inert), the delimiter
//!   name chosen by an `if` over the previous result;
//! - `stream`: `collect ... if g` stream claims, `g` defined inside the
//!   stream from the previous result.
//!
//! Prints the interpreter time, cold time at 1 and 16 threads with the
//! critical path, and three edits (a value in the middle, which the early
//! cutoff should stop at once; a delimiter moved, which grows one claim;
//! the delimiter choice broken, which claims the whole rest and back).
//!
//! `cargo run --release --example chains -- [N..] [--threads T] [--rounds R] [--print N]`

mod common;

use std::time::{Duration, Instant};

use common::{Program, Threads, interp::interpret};
use phitex_defuse::{Engine, Greedy, Report, Speculation};

/// An edit kind: round number to (line index, new text).
type EditFn = Box<dyn Fn(usize) -> (usize, String)>;

fn inert_chain(n: usize) -> Vec<String> {
    let mut l = Vec::with_capacity(5 * n);
    for i in 0..n {
        if i == 0 {
            l.push("s0 = \"a\"".into());
        } else {
            l.push(format!(
                "if x{} == x{} {{ global s{i} = \"a\" }} else {{ global s{i} = \"b\" }}",
                i - 1,
                i - 1
            ));
        }
        l.push(format!("listuntil l{i} s{i}"));
        l.push(format!("{}", 10 + i));
        l.push("mark a".into());
        l.push(format!("x{i} = l{i}"));
    }
    l
}

fn stream_chain(n: usize) -> Vec<String> {
    let mut l = Vec::with_capacity(5 * n);
    for i in 0..n {
        l.push(format!("collect c{i} until mark if g{i}"));
        if i == 0 {
            l.push("g0 = true".into());
        } else {
            l.push(format!("g{i} = x{} == x{}", i - 1, i - 1));
        }
        l.push(format!("{}", 10 + i));
        l.push("mark a".into());
        l.push(format!("x{i} = c{i}"));
    }
    l
}

fn eval(eng: &mut Engine, threads: usize) -> (Duration, Report) {
    let t = Instant::now();
    // `SPEC=off|predicted|all` picks the speculation of the threaded runs.
    let spec = match std::env::var("SPEC").as_deref() {
        Ok("off") => Speculation::Off,
        Ok("all") => Speculation::All,
        _ => Speculation::Predicted,
    };
    let r = if threads <= 1 {
        eng.evaluate_sequential().expect("evaluate")
    } else {
        eng.evaluate(&mut Greedy::speculative(spec, 256), &mut Threads(threads))
            .expect("evaluate")
    };
    (t.elapsed(), r)
}

fn pct(ts: &mut [Duration], p: usize) -> Duration {
    ts.sort();
    ts[(ts.len() - 1) * p / 100]
}

fn bench(name: &str, lines: Vec<String>, threads: &[usize], rounds: usize) {
    let n = lines.len() / 5;
    let ti = Instant::now();
    let reference = interpret(&lines).expect("interpreter");
    let interp = ti.elapsed();
    println!(
        "== {name} chain, {n} links ({} lines): interpreter {interp:.3?}",
        lines.len()
    );
    for &t in threads {
        let (mut eng, prog) = Program::load(lines.clone());
        let (d, r) = eval(&mut eng, t);
        assert_eq!(
            prog.result(&eng),
            reference,
            "{name} {n}: cold result differs from the interpreter"
        );
        println!(
            "threads {t:>2}  cold {d:>10.3?} ({:.2}x interpreter)  runs {}  steps {}  pending waits {}  critical path {}",
            d.as_secs_f64() / interp.as_secs_f64(),
            r.runs,
            r.steps,
            r.pending_waits,
            eng.critical_path()
        );
    }
    let threads = *threads.last().unwrap_or(&1);
    let (mut eng, mut prog) = Program::load(lines);
    eval(&mut eng, threads);
    let mid = n / 2;
    let kinds: [(&str, EditFn); 3] = [
        (
            "value in the middle",
            Box::new(move |k| (5 * mid + 2, format!("{}", 1000 + k))),
        ),
        (
            "delimiter moved",
            Box::new(move |k| (5 * mid + 3, if k % 2 == 0 { "mark b".into() } else { "mark a".into() })),
        ),
        (
            "choice broken",
            Box::new(move |k| {
                let s = if k % 2 == 0 { "b" } else { "a" };
                (
                    5 * mid,
                    format!(
                        "if x{} == x{} {{ global s{mid} = \"{s}\" }} else {{ global s{mid} = \"{s}\" }}",
                        mid - 1,
                        mid - 1
                    ),
                )
            }),
        ),
    ];
    println!(
        "{:<22} {:>5} {:>11} {:>11} {:>8} {:>8}",
        "edit", "n", "p50", "p99", "run~", "waits~"
    );
    for (label, edit) in &kinds {
        if *label == "choice broken" && name != "inert" {
            continue;
        }
        let mut ts = Vec::new();
        let mut runs = Vec::new();
        let mut waits = Vec::new();
        for k in 0..rounds {
            let (i, text) = edit(k);
            prog.set_line(&mut eng, i, text);
            if phitex_defuse::obs::enabled() {
                phitex_defuse::obs::reset();
            }
            let (d, r) = eval(&mut eng, threads);
            if phitex_defuse::obs::enabled() && k == 0 {
                // Profiling builds: the engine's counters for the first round.
                print!("probes ({label}, first round):\n{}", phitex_defuse::obs::snapshot());
                println!("report: {r:?}");
            }
            assert_eq!(
                prog.result(&eng),
                interpret(&prog.lines).expect("interpreter"),
                "{name} {n} {label} round {k}"
            );
            ts.push(d);
            runs.push(r.nodes_run);
            waits.push(r.pending_waits);
        }
        runs.sort_unstable();
        waits.sort_unstable();
        println!(
            "{label:<22} {:>5} {:>11.3?} {:>11.3?} {:>8} {:>8}",
            rounds,
            pct(&mut ts, 50),
            pct(&mut ts, 99),
            runs[runs.len() / 2],
            waits[waits.len() / 2]
        );
    }
}

fn main() {
    let mut sizes = Vec::new();
    let mut threads = vec![1, 16];
    let mut rounds = 10;
    let mut print = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--threads" => threads = vec![1, args.next().and_then(|s| s.parse().ok()).expect("--threads T")],
            "--rounds" => rounds = args.next().and_then(|s| s.parse().ok()).expect("--rounds R"),
            "--print" => print = Some(args.next().and_then(|s| s.parse().ok()).expect("--print N")),
            s => sizes.push(s.parse::<usize>().expect("size")),
        }
    }
    if let Some(n) = print {
        for l in inert_chain(n) {
            println!("{l}");
        }
        println!("--");
        for l in stream_chain(n) {
            println!("{l}");
        }
        return;
    }
    if sizes.is_empty() {
        sizes = vec![100, 4000];
    }
    for &n in &sizes {
        bench("inert", inert_chain(n), &threads, rounds);
        bench("stream", stream_chain(n), &threads, rounds);
    }
}
