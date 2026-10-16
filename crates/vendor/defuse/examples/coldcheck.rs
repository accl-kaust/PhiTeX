//! Whether a program evaluates right from cold under a schedule: exit 1
//! when it does not (`coldcheck PROGRAM.txt seq|spec|random`).
#[path = "common/mod.rs"]
mod common;

use common::Program;
use common::shrink::{Sched, ok};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("PROGRAM.txt");
    let sched = match args.next().as_deref() {
        Some("seq") => Sched::Seq,
        Some("random") => Sched::Random(1),
        _ => Sched::Spec(3),
    };
    let lines: Vec<String> = std::fs::read_to_string(path)
        .expect("read")
        .lines()
        .map(str::to_string)
        .collect();
    let (mut eng, p) = Program::load(lines);
    if !ok(&mut eng, &p, sched) {
        println!("{:?}", p.result(&eng));
        for &n in &p.nodes {
            println!("{}", eng.debug_node(n));
            println!("{}", eng.debug_reads(n));
        }
        println!("stale {:?}", eng.check_reads());
        std::process::exit(1);
    }
}
