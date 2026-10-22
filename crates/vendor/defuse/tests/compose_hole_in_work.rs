//! A composition refiled while one of its holes' definers is in work: the
//! other holes take their new values then. Edited together, `x` gets a new
//! definer before the pack and `l` one in work; the commit of `x`'s refiles
//! the pack with `l`'s answer not there, and `l`'s commit later refiles it
//! on `l` only. Taken as all or nothing, `x`'s new value was lost (the
//! fuzzer's paths pool, views and batches, seed 905).

#[path = "../examples/common/mod.rs"]
mod common;
use common::shrink::{Ed, Sched, replay_bad};

#[test]
fn a_hole_waiting_on_its_definer_keeps_the_others_new() {
    // SAFETY: the one test in this binary, before any thread reads them.
    unsafe {
        std::env::set_var("FUZZ_PATHS", "2");
        std::env::set_var("FUZZ_BATCH", "3");
    }
    let start: Vec<String> = ["x = 3", "global l = [2, 2]", "s = \"abc\"", "print y", "pack z = (l[1], x), s"].iter().map(|s| s.to_string()).collect();
    let eds = [Ed::Set(1, "x = 4".into()), Ed::Set(3, "l = [4, 2, 3]".into())];
    for sched in [Sched::Seq, Sched::Par(4), Sched::Spec(4)] {
        assert!(!replay_bad(&start, &eds, sched), "{sched:?}");
    }
}
