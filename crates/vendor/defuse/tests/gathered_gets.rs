//! What a run takes by index through a view (`Ctx::get`) is read as one
//! read of the view, at the next mark and at the run's end: the run
//! depends on exactly the elements it took. The toy reads `l[i]` through a
//! view under `FUZZ_PATHS=2`; `run { .. }` marks each assignment.

#[path = "../examples/common/mod.rs"]
mod common;
use common::Program;
use common::interp::interpret;
use common::shrink::{Ed, apply};

fn lines(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

fn views() {
    // SAFETY: every test sets the same value, before any thread reads it.
    unsafe { std::env::set_var("FUZZ_PATHS", "2") };
}

#[test]
fn an_element_not_taken_changes_and_the_taker_stands() {
    views();
    let (mut e, mut p) = Program::load(lines(&["l = [1, 2, 3]", "c = l[0] + l[2]", "print c"]));
    e.evaluate_sequential().unwrap();
    apply(&mut e, &mut p, &Ed::Set(0, "l = [1, 9, 3]".into()));
    let rep = e.evaluate_sequential().unwrap();
    assert_eq!(rep.nodes_run, 1, "{rep:?}");
    for (k, l) in ["l = [5, 9, 3]", "l = [5, 9, 4]", "l = [5, 9]", "l = [5, 9, 6, 7]"].into_iter().enumerate() {
        apply(&mut e, &mut p, &Ed::Set(0, l.into()));
        e.evaluate_sequential().unwrap();
        assert_eq!(p.result(&e), interpret(&p.lines).unwrap(), "edit {k}");
        assert!(e.check_reads().is_empty(), "{:?}", e.check_reads());
    }
}

#[test]
fn elements_taken_before_a_mark_are_read_before_it() {
    views();
    let (mut e, mut p) = Program::load(lines(&["l = [1, 2, 3]", "run { a = l[0] + 0  b = l[2] + a  c = b * 2 }", "print c"]));
    e.evaluate_sequential().unwrap();
    // (Element 0, taken before the second mark: resuming there would keep
    // the old `a`.)
    apply(&mut e, &mut p, &Ed::Set(0, "l = [7, 2, 3]".into()));
    e.evaluate_sequential().unwrap();
    assert_eq!(p.result(&e), interpret(&p.lines).unwrap());
    assert!(e.check_reads().is_empty(), "{:?}", e.check_reads());
    // Element 2, taken after it: resumed there.
    apply(&mut e, &mut p, &Ed::Set(0, "l = [7, 2, 8]".into()));
    let rep = e.evaluate_sequential().unwrap();
    assert_eq!(rep.resumed_at_mark, 1, "{rep:?}");
    assert_eq!(p.result(&e), interpret(&p.lines).unwrap());
    assert!(e.check_reads().is_empty(), "{:?}", e.check_reads());
}
