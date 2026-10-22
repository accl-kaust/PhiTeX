//! A range reader whose window's start moves (the page before it removed)
//! stays filed: its old entry and its new one differ in the start only,
//! and refiling the new one before dropping the old one must not drop it
//! (the fuzz's loop pool found a `page` left unasked when the `emit`
//! before it changed).

#[path = "../examples/common/mod.rs"]
mod common;
use common::Program;
use common::shrink::{Ed, apply};

#[test]
fn a_range_readers_moved_window_stays_filed() {
    let lines: Vec<String> = ["loop a = 0", "a = 3", "loop c = 0", "emit c", "page", "page", "x = a + b + c", "c = a + b"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let (mut e, mut p) = Program::load(lines);
    e.evaluate_sequential().unwrap();
    for ed in [Ed::Rm(4), Ed::Rm(6)] {
        apply(&mut e, &mut p, &ed);
        e.evaluate_sequential().unwrap();
        assert_eq!(e.check_reads(), Vec::<String>::new(), "after {ed:?}");
        assert_eq!(p.result(&e), common::interp::interpret(&p.lines).unwrap(), "after {ed:?}");
    }
}
