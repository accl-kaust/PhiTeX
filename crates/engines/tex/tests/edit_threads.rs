//! An edit that begins with a few nodes and grows takes the pool's
//! threads: before, an edit whose text changed few nodes ran on the
//! calling thread alone to its end, and a one-letter edit of amsldoc that
//! went on to rerun 1275 nodes took 26 s on one thread of 24.

mod common;

use std::fmt::Write as _;

use phitex_tex::Options;

#[test]
fn an_edit_that_grows_runs_on_the_pool() {
    let dir = common::scratch("edit-threads");
    let opts = Options {
        threads: Some(8),
        ..common::options("./d.tex", "d", "", &dir)
    };
    // (every paragraph reads \x: the edit changes one line, the
    // definition, and every paragraph runs again)
    let doc = |x: &str| {
        let mut s = format!("\\def\\x{{{x}}}\n\n");
        for i in 0..300 {
            let _ = write!(
                s,
                "Paragraph {i} says \\x\\ and goes on long enough to make a line or two in the plain format's ten point roman.\n\n"
            );
        }
        s.push_str("\\bye\n");
        s
    };
    let mut b = common::build(opts, Some(&common::plain()), &doc("one"));
    let cold = b.output().0.clone();
    assert!(!cold.is_empty(), "the cold build made output");
    b.edit(&doc("another word"));
    let s = &b.stats;
    assert!(
        s.nodes_run >= 300,
        "the edit reran every paragraph: {} nodes run",
        s.nodes_run
    );
    assert_eq!(
        s.threads, 8,
        "an edit of a few nodes that grew to {} ran on {} thread(s)",
        s.nodes_run, s.threads
    );
}
