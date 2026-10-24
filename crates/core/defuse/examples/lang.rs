//! A tour of the example language: cold evaluation, then edits with their
//! incremental reports. `cargo run --example lang`

mod common;

use common::Program;
use phitex_defuse::{Greedy, Sequential, Speculation};

const SRC: &[&str] = &[
    "loop total = 0",
    "base = 10",
    "p = {x: 3, w: 4, color: 7}",
    "edge = layout(p)",
    "scale = fn(a, b) { t = a * b  return t + base }",
    "s1 = scale(2, 5)",
    "proc setg(a) { global gg = a * 2 }",
    "do setg(base)",
    "fromproc = gg + 1",
    "begin",
    "inner = base + 1",
    "global shared = inner * 2",
    "end",
    "outer = base + shared",
    "meaning put = \"set\"",
    "~put made outer + 1",
    "sumnext two 2",
    "base * 2",
    "3",
    "listto items",
    "edge",
    "two",
    "endlist",
    "emit base",
    "emit two",
    "page",
    "early = total + 1",
    "total = s1 + edge",
    "print early",
];

fn main() {
    let (mut eng, mut prog) = Program::load(SRC.iter().map(|s| (*s).to_string()).collect());
    let mut sched = Greedy::speculative(Speculation::Predicted, 64);
    let r = eng.evaluate(&mut sched, &mut Sequential).expect("evaluate");
    println!("cold: {r:?}");
    show(&eng, &prog);
    let edits: &[(&str, usize, &str)] = &[
        ("record field not read by layout", 2, "p = {x: 3, w: 4, color: 8}"),
        ("a definition used far away", 1, "base = 11"),
        ("meaning of a name", 14, "meaning put = \"double\""),
        ("inside a claimed range", 18, "4"),
        ("the delimiter of a claim", 22, "base"),
    ];
    for (what, line, text) in edits {
        prog.set_line(&mut eng, *line, (*text).to_string());
        let r = eng.evaluate(&mut sched, &mut Sequential).expect("evaluate");
        println!("\n{what}: `{text}`\n  {r:?}");
        show(&eng, &prog);
    }
}

fn show(eng: &phitex_defuse::Engine, prog: &Program) {
    for (line, out) in prog
        .lines
        .iter()
        .chain(std::iter::once(&"<out>".to_string()))
        .zip(prog.render(eng))
    {
        println!("  {line:<48} {out}");
    }
}
