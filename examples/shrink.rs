//! Minimises a failing edit case: a program (one statement per line) and
//! an edits file (`set I|TEXT`, `ins I|TEXT`, `rm I`, applied in order)
//! whose last edit makes the incremental result differ from the
//! interpreter, fail to evaluate, or break an invariant.
//!
//! `cargo run --release --example shrink -- PROGRAM.txt EDITS.txt
//!     [--sched seq|spec|spec:T|random] [--out DIR]`
//!
//! The typing benchmark writes `target/fail/before.txt` and
//! `target/fail/edits.txt` when a checked edit fails, so
//! `shrink target/fail/before.txt target/fail/edits.txt` is the usual call.
//! The minimal case goes to `DIR/min.txt` and `DIR/min-edits.txt`
//! (default `target/fail`).

mod common;

use common::shrink::{self, Sched};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut paths = Vec::new();
    let mut sched: Option<Sched> = None;
    let mut out = "target/fail".to_string();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--sched" => {
                sched = Some(
                    args.next()
                        .and_then(|s| Sched::parse(&s))
                        .expect("--sched seq|spec|spec:T|random"),
                )
            }
            "--out" => out = args.next().expect("--out DIR"),
            s => paths.push(s.to_string()),
        }
    }
    let [prog, edits] = paths.as_slice() else {
        eprintln!("usage: shrink PROGRAM.txt EDITS.txt [--sched seq|spec|spec:T|random] [--out DIR]");
        std::process::exit(2);
    };
    let start: Vec<String> = std::fs::read_to_string(prog)
        .unwrap_or_else(|e| {
            eprintln!("{prog}: {e}");
            std::process::exit(2);
        })
        .lines()
        .map(String::from)
        .collect();
    let eds = shrink::parse_edits(&std::fs::read_to_string(edits).unwrap_or_else(|e| {
        eprintln!("{edits}: {e}");
        std::process::exit(2);
    }))
    .unwrap_or_else(|e| {
        eprintln!("{edits}: {e}");
        std::process::exit(1);
    });
    // The scheduler that reproduces it: the requested one, else any.
    let candidates: Vec<Sched> = sched.map_or_else(|| Sched::ALL.to_vec(), |s| vec![s]);
    let Some(sched) = candidates.into_iter().find(|&s| {
        let hit = shrink::replay_bad(&start, &eds, s);
        println!("{s:?}: {}", if hit { "reproduces" } else { "does not reproduce" });
        hit
    }) else {
        println!("not reproduced from a cold load of the program: the failure needs the earlier history");
        std::process::exit(1);
    };
    let (min, eds) = shrink::minimize(start, eds, sched, &|lines, edits| {
        eprintln!("{lines} lines, {edits} edits")
    });
    let _ = std::fs::create_dir_all(&out);
    std::fs::write(format!("{out}/min.txt"), min.join("\n")).expect("write");
    std::fs::write(format!("{out}/min-edits.txt"), shrink::edits_text(&eds)).expect("write");
    println!(
        "MIN {} lines, {} edits (written to {out}/min.txt, {out}/min-edits.txt), under {sched:?}",
        min.len(),
        eds.len()
    );
    shrink::report(&min, &eds, sched);
}
