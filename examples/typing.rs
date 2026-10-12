//! Typing benchmark: programs of 1k / 10k / 100k / 1M lines, cold
//! evaluation, then single-edit replays, each checked bytewise against a cold
//! evaluation (unless `--no-check`).
//!
//! `cargo run --release --example typing [sizes..] [--edits N] [--threads N]
//!  [--no-check] [--scaling] [--bounded] [--slow] [--trace FILE]`
//!
//! Built with `--profile profiling --features profile`, it also prints the
//! engine's probe counters and span times (`defuse::obs`) after the cold
//! evaluation and after the edits, and `--trace FILE` writes a Chrome trace
//! (open in <https://ui.perfetto.dev>).
#![allow(clippy::many_single_char_names)]

mod common;

use std::time::{Duration, Instant};

use common::{Program, Threads, generate_with};
use defuse::{Engine, Greedy, Report, Rng, Speculation};

#[allow(clippy::struct_excessive_bools)]
struct Opts {
    edits: usize,
    threads: usize,
    check: bool,
    scaling: bool,
    /// Generate without evaluated text that starts with a computed piece.
    bounded: bool,
    /// Print each edit slower than 50 ms.
    slow: bool,
    trace: Option<String>,
}

fn main() {
    let mut sizes = Vec::new();
    let mut o = Opts {
        edits: 5,
        threads: std::thread::available_parallelism().map_or(4, std::num::NonZero::get),
        check: true,
        scaling: false,
        bounded: false,
        slow: false,
        trace: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--edits" => o.edits = args.next().and_then(|s| s.parse().ok()).expect("--edits N"),
            "--threads" => o.threads = args.next().and_then(|s| s.parse().ok()).expect("--threads N"),
            "--no-check" => o.check = false,
            "--scaling" => o.scaling = true,
            "--bounded" => o.bounded = true,
            "--slow" => o.slow = true,
            "--trace" => o.trace = Some(args.next().expect("--trace FILE")),
            s => sizes.push(s.parse::<usize>().expect("size")),
        }
    }
    if sizes.is_empty() {
        sizes = vec![1_000, 10_000, 100_000, 1_000_000];
    }
    let (node, edge) = Engine::record_sizes();
    println!("node record {node} B, use edge {edge} B");
    defuse::obs::set_tracing(o.trace.is_some());
    for n in sizes {
        if o.scaling {
            scaling(n, o.bounded);
        } else {
            bench(n, &o);
        }
    }
    if let Some(path) = &o.trace {
        std::fs::write(path, defuse::obs::chrome_trace()).expect("write trace");
        println!("trace written to {path}");
    }
}

/// Prints and resets the engine's probes (profiling builds only).
fn probes(what: &str) {
    if defuse::obs::enabled() {
        print!("probes ({what}):\n{}", defuse::obs::snapshot());
        defuse::obs::reset();
    }
}

fn eval(eng: &mut Engine, threads: usize, spec: bool) -> (Duration, Report) {
    let mut sched = if spec {
        Greedy::speculative(Speculation::Predicted, 256)
    } else {
        Greedy::default()
    };
    let t = Instant::now();
    let r = eng.evaluate(&mut sched, &mut Threads(threads)).expect("evaluate");
    (t.elapsed(), r)
}

fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|x| x.parse::<f64>().ok()))
        .map_or(0.0, |pages| pages * 4096.0 / 1e6)
}

fn check(prog: &Program, eng: &Engine) -> Duration {
    let t = Instant::now();
    let want = common::interp::interpret(&prog.lines).expect("interpreter");
    let got = prog.result(eng);
    if want != got {
        let i = want
            .iter()
            .zip(&got)
            .position(|(a, b)| a != b)
            .unwrap_or(want.len().min(got.len()));
        let _ = std::fs::create_dir_all("target/fail");
        let _ = std::fs::write("target/fail/lines.txt", prog.lines.join("\n"));
        let rendered = prog.render(eng);
        panic!(
            "incremental result differs from the interpreter: first at line {i} {:?}: engine {:?}; program written to target/fail/lines.txt ({} lines)",
            prog.lines.get(i),
            rendered.get(i),
            prog.lines.len()
        );
    }
    t.elapsed()
}

fn scaling(n: usize, bounded: bool) {
    println!(
        "== cold thread scaling, {n} lines{}",
        if bounded { " (no unbounded evals)" } else { "" }
    );
    let lines = generate_with(42, n, !bounded);
    let ti = Instant::now();
    let reference = common::interp::interpret(&lines).expect("interpreter");
    println!("interpreter {:.3?}", ti.elapsed());
    let mut base = None;
    for t in [1, 2, 4, 8, 16] {
        let tl = Instant::now();
        let (mut eng, prog) = Program::load(lines.clone());
        let load = tl.elapsed();
        let (d, r) = eval(&mut eng, t, false);
        let b = *base.get_or_insert(d);
        assert_eq!(reference, prog.result(&eng), "cold result differs from the interpreter");
        println!(
            "threads {t:>2}  load {load:>9.3?}  cold {d:>10.3?}  speedup {:>5.2}  runs {}  steps {}  critical path {}  rss {:.0} MB",
            b.as_secs_f64() / d.as_secs_f64(),
            r.runs,
            r.steps,
            eng.critical_path(),
            rss_mb()
        );
    }
}

const KINDS: [&str; 7] = [
    "insert char",
    "delete char",
    "change far def",
    "insert def",
    "redefine meaning",
    "edit in claim",
    "move block close",
];

fn bench(n: usize, o: &Opts) {
    println!("== {n} lines ({} threads)", o.threads);
    let t = Instant::now();
    let (mut eng, mut prog) = Program::load(generate_with(42, n, !o.bounded));
    let load = t.elapsed();
    probes("load");
    let (t, r) = eval(&mut eng, o.threads, false);
    probes("cold evaluation");
    let ti = Instant::now();
    let reference = common::interp::interpret(&prog.lines).expect("interpreter");
    let interp = ti.elapsed();
    if o.check {
        assert_eq!(reference, prog.result(&eng), "cold result differs from the interpreter");
    }
    println!(
        "interpreter {interp:.3?}  engine load+cold {:.3?} ({:.2}x)",
        load + t,
        (load + t).as_secs_f64() / interp.as_secs_f64()
    );
    println!(
        "load {load:.3?}  cold {t:.3?}  runs {}  stops {}  gated {}  pending waits {}  live {}  critical path {}  rss {:.0} MB",
        r.runs,
        r.stops,
        r.gated,
        r.pending_waits,
        eng.live_nodes(),
        eng.critical_path(),
        rss_mb()
    );
    println!(
        "{:<17} {:>10}  {:>6} {:>6} {:>6} {:>6} {:>6}",
        "edit", "wall", "run", "cut", "rebnd", "waits", "execs"
    );
    let mut rng = Rng::new(n as u64);
    let mut stats: Vec<Vec<(Duration, Report)>> = vec![Vec::new(); KINDS.len()];
    for round in 0..o.edits {
        for (kind, name) in KINDS.iter().enumerate() {
            if !edit(&mut rng, &mut eng, &mut prog, kind, round) {
                continue;
            }
            let (t, r) = eval(&mut eng, o.threads, true);
            if o.check {
                eprintln!("checking {name} round {round}");
                let c = check(&prog, &eng);
                println!(
                    "{name:<17} {t:>10.3?}  {:>6} {:>6} {:>6} {:>6} {:>6}  ok ({c:.0?})",
                    r.nodes_run, r.cutoffs, r.rebinds, r.pending_waits, r.runs
                );
            }
            if o.slow && t > Duration::from_millis(50) {
                println!(
                    "slow {name:<12} round {round:>3} {t:>10.3?}  run {} cut {} rebnd {} waits {} execs {} rounds {} region {}",
                    r.nodes_run, r.cutoffs, r.rebinds, r.pending_waits, r.runs, r.rounds, r.scope_region
                );
            }
            stats[kind].push((t, r));
        }
    }
    probes(if o.check {
        "edits, including their cold checks"
    } else {
        "edits"
    });
    println!(
        "{:<17} {:>5} {:>11} {:>11} {:>8} {:>8} {:>8}",
        "summary", "n", "median", "p99", "run~", "rebnd~", "waits~"
    );
    for (kind, name) in KINDS.iter().enumerate() {
        let s = &mut stats[kind];
        if s.is_empty() {
            continue;
        }
        let mut ts: Vec<Duration> = s.iter().map(|x| x.0).collect();
        ts.sort();
        let pct = |p: usize| ts[(ts.len() - 1) * p / 100];
        let med = |f: &dyn Fn(&Report) -> u64| {
            let mut v: Vec<u64> = s.iter().map(|x| f(&x.1)).collect();
            v.sort_unstable();
            v[v.len() / 2]
        };
        println!(
            "{name:<17} {:>5} {:>11.3?} {:>11.3?} {:>8} {:>8} {:>8}",
            s.len(),
            pct(50),
            pct(99),
            med(&|r| r.nodes_run),
            med(&|r| r.rebinds),
            med(&|r| r.pending_waits)
        );
    }
}

fn find(rng: &mut Rng, prog: &Program, pred: &dyn Fn(usize, &str) -> bool) -> Option<usize> {
    let n = prog.lines.len();
    for _ in 0..10_000 {
        let i = rng.below(n);
        if pred(i, &prog.lines[i]) {
            return Some(i);
        }
    }
    None
}

fn bump_last_digit(l: &str) -> Option<String> {
    let p = l.rfind(|c: char| c.is_ascii_digit())?;
    let mut s = l.to_string();
    let d = s.as_bytes()[p];
    s.replace_range(p..=p, &char::from(if d == b'9' { b'0' } else { d + 1 }).to_string());
    Some(s)
}

fn edit(rng: &mut Rng, eng: &mut Engine, prog: &mut Program, kind: usize, round: usize) -> bool {
    let header = 40;
    match kind {
        0 => {
            let Some(i) = find(rng, prog, &|i, l| {
                i >= header && l.contains(|c: char| c.is_ascii_digit())
            }) else {
                return false;
            };
            let mut l = prog.lines[i].clone();
            let p = l.rfind(|c: char| c.is_ascii_digit()).expect("digit");
            l.insert(p + 1, '1');
            prog.set_line(eng, i, l);
        }
        1 => {
            let Some(i) = find(rng, prog, &|i, l| {
                let b = l.as_bytes();
                i >= header
                    && l.rfind(|c: char| c.is_ascii_digit())
                        .is_some_and(|p| p > 0 && b[p - 1].is_ascii_digit())
            }) else {
                return false;
            };
            let mut l = prog.lines[i].clone();
            let p = l.rfind(|c: char| c.is_ascii_digit()).expect("digit");
            l.remove(p);
            prog.set_line(eng, i, l);
        }
        2 => {
            // A header constant, read from sections all over the program.
            let i = 8 + (round * 7) % 32;
            let Some(l) = bump_last_digit(&prog.lines[i]) else {
                return false;
            };
            prog.set_line(eng, i, l);
        }
        3 => {
            let Some(i) = find(rng, prog, &|i, l| {
                i >= header && l.starts_with('x') && l.contains(" = ")
            }) else {
                return false;
            };
            let name = prog.lines[i].split(' ').next().expect("name").to_string();
            prog.insert_line(eng, i + 1, format!("{name} = 7"));
        }
        4 => {
            let Some(i) = find(rng, prog, &|i, l| i >= header && l.starts_with("meaning put")) else {
                return false;
            };
            let l = if prog.lines[i].contains("set") {
                "meaning put = \"double\""
            } else {
                "meaning put = \"set\""
            };
            prog.set_line(eng, i, l.to_string());
        }
        5 => {
            let Some(i) = find(rng, prog, &|i, l| {
                i >= header
                    && (prog.lines[i - 1].starts_with("sumnext") || prog.lines[i - 1].starts_with("listto"))
                    && l.contains(|c: char| c.is_ascii_digit())
            }) else {
                return false;
            };
            let Some(l) = bump_last_digit(&prog.lines[i]) else {
                return false;
            };
            prog.set_line(eng, i, l);
        }
        _ => {
            // Move a block's close one line earlier.
            let Some(i) = find(rng, prog, &|i, l| {
                i > header && l == "end" && !prog.lines[i - 1].is_empty()
            }) else {
                return false;
            };
            // The new close first: the old one is then unmatched (ignored),
            // so neither step re-binds past the block. (Removing first would
            // re-nest every later block until the insert.)
            prog.insert_line(eng, i - 1, "end".into());
            prog.remove_line(eng, i + 1);
        }
    }
    true
}
