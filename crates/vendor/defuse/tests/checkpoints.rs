//! Rerun from the first changed read (`Ctx::checkpoint`, `keep_after`,
//! `resumed`): a run whose read after a mark broke resumes at the mark;
//! one that reaches a mark of its last run with the same state and
//! outputs is cut off there, its last result standing. The toy's
//! `run { .. }` marks each assignment.

#[path = "../examples/common/mod.rs"]
mod common;
use common::Program;
use common::interp::interpret;
use common::shrink::{Ed, apply};

fn lines(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_run_whose_state_comes_out_the_same_is_cut_off() {
    let (mut e, mut p) = Program::load(lines(&["x = 1", "run { a = x * 0 + 4  b = a + 1  c = b * 2 }", "print c"]));
    e.evaluate_sequential().unwrap();
    apply(&mut e, &mut p, &Ed::Set(0, "x = 2".into()));
    let rep = e.evaluate_sequential().unwrap();
    assert_eq!((rep.resumed_at_mark, rep.cut_in_run), (1, 1), "{rep:?}");
    assert_eq!(p.result(&e), interpret(&p.lines).unwrap());
    assert!(e.check_reads().is_empty(), "{:?}", e.check_reads());
}

#[test]
fn a_run_whose_state_changes_resumes_and_runs_to_its_end() {
    let (mut e, mut p) = Program::load(lines(&["x = 1", "y = 5", "run { a = y + 1  b = x + a  c = b * 2 }", "print c"]));
    e.evaluate_sequential().unwrap();
    // (`x` is read at the second assignment: resumed there.)
    apply(&mut e, &mut p, &Ed::Set(0, "x = 2".into()));
    let rep = e.evaluate_sequential().unwrap();
    assert_eq!((rep.resumed_at_mark, rep.cut_in_run), (1, 0), "{rep:?}");
    assert_eq!(p.result(&e), interpret(&p.lines).unwrap());
    // And again, from the first: `y` read at the first.
    apply(&mut e, &mut p, &Ed::Set(1, "y = 6".into()));
    let rep = e.evaluate_sequential().unwrap();
    assert_eq!(rep.resumed_at_mark, 1, "{rep:?}");
    assert_eq!(p.result(&e), interpret(&p.lines).unwrap());
    assert!(e.check_reads().is_empty(), "{:?}", e.check_reads());
}
