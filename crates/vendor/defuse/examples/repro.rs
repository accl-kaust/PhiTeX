//! Repeats a fuzz history: a program (one statement per line) and an
//! edits file (`set I|TEXT`, `ins I|TEXT`, `rm I`; `fuzz` writes both with
//! `FUZZ_DUMP`), under a scheduler, `N` times, checking after every edit;
//! prints how many tries failed and where (with `FUZZ_WHY`, which check).
//!
//! `cargo run --release --example repro -- PROGRAM.txt EDITS.txt N [par|spec|seq] [noeval]`

mod common;

use common::shrink::{self, Ed, Sched};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let prog: Vec<String> = std::fs::read_to_string(&a[0])
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    let eds: Vec<Ed> = std::fs::read_to_string(&a[1])
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (op, rest) = l.split_once(' ').unwrap();
            let (i, t) = rest.split_once('|').unwrap_or((rest, ""));
            let i: usize = i.parse().unwrap();
            match op {
                "set" => Ed::Set(i, t.to_string()),
                "ins" => Ed::Ins(i, t.to_string()),
                _ => Ed::Rm(i),
            }
        })
        .collect();
    let n: usize = a[2].parse().unwrap();
    let sched = match a.get(3).map(String::as_str) {
        Some("spec") => Sched::Spec(3),
        Some("seq") => Sched::Seq,
        _ => Sched::Par(4),
    };
    common::REPLAY_STOPS.store(true, std::sync::atomic::Ordering::Relaxed);
    // `noeval` (fifth argument): every edit applied unevaluated, one
    // evaluation at the end.
    if a.get(4).map(String::as_str) == Some("noeval") {
        let (mut eng, mut p) = common::Program::load(prog.clone());
        for e in &eds {
            shrink::apply(&mut eng, &mut p, e);
        }
        println!("end ok: {}", shrink::ok(&mut eng, &p, sched));
        return;
    }
    // Each try: load, then every edit, checking after each (as the fuzz
    // driver does); a failing check reports the edit's index.
    let mut bad = 0;
    for k in 0..n {
        let (mut eng, mut p) = common::Program::load(prog.clone());
        if !shrink::ok(&mut eng, &p, sched) {
            bad += 1;
            println!("try {k}: cold bad");
            continue;
        }
        for (j, e) in eds.iter().enumerate() {
            shrink::apply(&mut eng, &mut p, e);
            if !shrink::ok(&mut eng, &p, sched) {
                bad += 1;
                println!("try {k}: edit {j} bad: {e:?}");
                break;
            }
        }
    }
    println!("{bad}/{n} bad");
}
