//! Repeats one edit on a program under a schedule until it goes wrong:
//! `edithunt CASE.txt [seq|spec|par] [N]`, CASE.txt holding the program's
//! lines, a line `---`, then the edit (`rm I`, `set I TEXT`, `ins I TEXT`).
#[path = "common/mod.rs"]
mod common;

use common::Program;
use common::shrink::{Ed, Sched, apply, ok};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("CASE.txt");
    let sched = match args.next().as_deref() {
        Some("seq") => Sched::Seq,
        Some("par") => Sched::Par(4),
        Some("spec1") => Sched::Spec(1),
        _ => Sched::Spec(3),
    };
    let n: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(200);
    let text = std::fs::read_to_string(path).expect("read");
    let (prog, ed) = text.split_once("\n---\n").expect("---");
    let lines: Vec<String> = prog.lines().map(str::to_string).collect();
    let ed = ed.trim_end();
    let (k, rest) = ed.split_once(' ').expect("edit");
    let (i, t) = rest.split_once(' ').map_or((rest, ""), |(a, b)| (a, b));
    let i: usize = i.parse().expect("index");
    let e = match k {
        "rm" => Ed::Rm(i),
        "set" => Ed::Set(i, t.to_string()),
        _ => Ed::Ins(i, t.to_string()),
    };
    let mut bad = 0;
    for _ in 0..n {
        eprintln!("=== iter");
        let (mut eng, mut p) = Program::load(lines.clone());
        if !ok(&mut eng, &p, sched) {
            bad += 1;
            continue;
        }
        apply(&mut eng, &mut p, &e);
        if !ok(&mut eng, &p, sched) {
            bad += 1;
            if bad == 1 && std::env::var_os("DUMP").is_some() {
                for b in eng.check_invariants() {
                    println!("INV {b}");
                }
                for &nd in &p.nodes {
                    println!("{}\n{}", eng.debug_node(nd), eng.debug_reads(nd));
                }
                std::process::exit(1);
            }
        }
    }
    println!("{bad}/{n}");
    std::process::exit(i32::from(bad > 0));
}
