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

#[test]
fn a_run_differing_in_a_slot_never_read_again_is_cut_off_with_a_patch() {
    let (mut e, mut p) = Program::load(lines(&["x = 1", "y = 5", "run { a = x + 1  b = y * 2  c = b + 1 }", "print a", "print c"]));
    e.evaluate_sequential().unwrap();
    // (`a` differs at the second mark, read by none after it: cut off
    // there, patched. A patched result keeps the marks its last run's
    // token takes the client back to, up to the one resumed at: the next
    // edit runs to the end, and the one after is cut off again.)
    for (k, x, cut) in [(1, "x = 2", 1), (2, "x = 7", 0), (3, "x = 9", 1)] {
        apply(&mut e, &mut p, &Ed::Set(0, x.into()));
        let rep = e.evaluate_sequential().unwrap();
        assert_eq!((rep.resumed_at_mark, rep.cut_in_run, rep.cut_patched), (1, cut, cut), "edit {k}: {rep:?}");
        assert_eq!(p.result(&e), interpret(&p.lines).unwrap(), "edit {k}");
        assert!(e.check_reads().is_empty(), "{:?}", e.check_reads());
    }
    // `y`, read after: no patch; resumed at the second mark, to its end.
    apply(&mut e, &mut p, &Ed::Set(1, "y = 6".into()));
    let rep = e.evaluate_sequential().unwrap();
    assert_eq!((rep.resumed_at_mark, rep.cut_patched), (1, 0), "{rep:?}");
    assert_eq!(p.result(&e), interpret(&p.lines).unwrap());
}
