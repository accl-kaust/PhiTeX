//! A run that loops on a state that has moved since it read it stops at
//! its next check and runs again on the new state (`Ctx::tick`): it never
//! reaches the commit that would refute it, and a one-letter edit of
//! amsldoc at -j24 left a page's run going for minutes at 5 GB this way,
//! every other worker idle. Here the text after two pages waits in a TeX
//! loop for a macro only the output routine defines; TeX runs the routine
//! at the `\eject`, before the loop, which so never turns. A run of the
//! text ahead of the page's finisher reads it undefined and turns until
//! the finisher defines it.

mod common;

use std::time::{Duration, Instant};

use phitex_tex::Options;

const SRC: &str = "\\output={\\shipout\\box255 \\global\\advance\\count0 by1 \\global\\let\\ready=\\relax}\n\
Page one.\\vfill\\eject\n\
Page two.\\vfill\\eject\n\
\\count255=0 \\loop\\ifx\\ready\\undefined \\advance\\count255 by1 \\repeat\n\
\\immediate\\write16{loop done: page \\the\\count0, turns \\the\\count255}\n\
Done.\n\
\\bye\n";

#[test]
fn a_loop_on_a_value_the_output_routine_defines_ends() {
    for threads in [2, 8, 24] {
        let dir = common::scratch(&format!("stale-loop-{threads}"));
        let opts = Options {
            threads: Some(threads),
            // (past this the build ends stuck: the loop never stopped)
            stuck_after: Some(Duration::from_secs(60)),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let t = Instant::now();
        let b = common::build(opts, Some(&common::plain()), SRC);
        let took = t.elapsed();
        let (dvi, log) = b.output();
        let stuck: Vec<String> = b
            .diagnostics()
            .into_iter()
            .map(|d| d.message)
            .filter(|m| m.contains("stuck:"))
            .collect();
        assert!(stuck.is_empty(), "-j{threads}: {stuck:?} after {took:?}");
        assert!(
            log.contains("loop done: page 3, turns 0"),
            "-j{threads}: {log}"
        );
        assert!(!dvi.is_empty(), "-j{threads}: no output");
        assert!(took < Duration::from_secs(30), "-j{threads}: {took:?}");
    }
}
