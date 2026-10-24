//! A loop-back field whose last publication in text order wins
//! (`LoopOptions::last_wins`): several publishers, no conflict; a later
//! publisher inserted, or the last removed, moves it.

#[path = "../examples/common/mod.rs"]
mod common;
use common::Program;
use common::interp::interpret;
use common::shrink::{Ed, apply};

fn lines(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_last_publication_wins() {
    let src = lines(&["loop last n = 0", "n = 1", "print n", "n = 2", "x = n * 10", "print x"]);
    let (mut e, mut p) = Program::load(src);
    e.evaluate_sequential().unwrap();
    let n = e.values().field("n");
    assert_eq!(e.loop_value(n).and_then(|x| e.values().as_int(x)), Some(2));
    assert_eq!(p.result(&e), interpret(&p.lines).unwrap());
    for ed in [Ed::Ins(6, "n = 3".into()), Ed::Rm(6), Ed::Rm(3), Ed::Ins(1, "n = 9".into())] {
        apply(&mut e, &mut p, &ed);
        e.evaluate_sequential().unwrap();
        assert_eq!(p.result(&e), interpret(&p.lines).unwrap(), "after {ed:?}");
        assert!(e.check_reads().is_empty(), "{:?}", e.check_reads());
    }
    // (The last now `n = 1`, the one inserted before it not.)
    assert_eq!(e.loop_value(n).and_then(|x| e.values().as_int(x)), Some(1));
}
