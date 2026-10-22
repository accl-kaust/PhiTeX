//! Two early loop-backs read crosswise (`m` from `n`, `n` from `m`): their
//! moves are taken one publisher at a time, in text order, each one's
//! readers running before the next. Taken together, each moved on the
//! other's old value, and from these values the two swapped states for
//! good (the fuzzer's last-wins pool, early, seed 280).

#[path = "../examples/common/mod.rs"]
mod common;
use common::Program;
use common::shrink::{Ed, Sched, apply, ok};

#[test]
fn crosswise_early_moves_settle() {
    // SAFETY: the one test in this binary, before any thread reads it.
    unsafe { std::env::set_var("FUZZ_LOOP_EARLY", "1") };
    let src: Vec<String> = ["loop last n = 0", "loop last m = 0", "x = n + m", "print x", "m = n * 0 + 5", "n = z + 1", "m = 3"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let (mut e, mut p) = Program::load(src);
    assert!(ok(&mut e, &p, Sched::Seq), "cold");
    apply(&mut e, &mut p, &Ed::Set(6, "n = m + 1".into()));
    assert!(ok(&mut e, &p, Sched::Seq), "after the edit: a fixed point, as a cold evaluation's or another");
}
